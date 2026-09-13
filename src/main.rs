use clap::{Parser, Subcommand};
use lumend::config::Config;
use lumend::ipc::{self, Request, Response};
use lumend::paths;
use serde_json::Value;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    version,
    about = "Adaptive screen brightness for laptops without a light sensor"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the daemon (normally started by the systemd user unit)
    Run {
        #[arg(long, value_name = "FILE")]
        config: Option<PathBuf>,
        /// Log what would change instead of touching the backlight or saving anything
        #[arg(long)]
        dry_run: bool,
    },
    /// Show what the daemon is doing
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Explain the current brightness target
    Why {
        #[arg(long)]
        json: bool,
    },
    /// Stop adjusting brightness, for a number of minutes or until resumed
    Pause { minutes: Option<u64> },
    /// Resume after a pause
    Resume,
    /// Delete everything lumend has learned
    Forget {
        #[arg(long)]
        yes: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command.unwrap_or(Cmd::Status { json: false }) {
        Cmd::Run { config, dry_run } => run(config, dry_run),
        Cmd::Status { json } => query(Request::Status, json, print_status),
        Cmd::Why { json } => query(Request::Why, json, print_why),
        Cmd::Pause { minutes } => query(Request::Pause { minutes }, false, |_| match minutes {
            Some(m) => println!("paused for {m} minutes"),
            None => println!("paused until `lumend resume`"),
        }),
        Cmd::Resume => query(Request::Resume, false, |_| println!("resumed")),
        Cmd::Forget { yes: false } => {
            Err("this deletes every learned sample; run `lumend forget --yes` to confirm".into())
        }
        Cmd::Forget { yes: true } => query(Request::Forget, false, |_| {
            println!("all learned data deleted")
        }),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("lumend: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(config_path: Option<PathBuf>, dry_run: bool) -> Result<(), Box<dyn std::error::Error>> {
    let filter = tracing_subscriber::EnvFilter::try_from_env("LUMEND_LOG")
        .unwrap_or_else(|_| "lumend=info".into());
    let journald = std::env::var_os("JOURNAL_STREAM").is_some();
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false);
    if journald {
        builder.without_time().init();
    } else {
        builder.init();
    }
    let path = config_path.unwrap_or_else(Config::default_path);
    let config = Config::load(&path)?;
    lumend::daemon::run(config, dry_run)
}

fn query(
    request: Request,
    raw: bool,
    print: impl FnOnce(&Value),
) -> Result<(), Box<dyn std::error::Error>> {
    let path = paths::socket_path();
    let response: Response = ipc::request(&path, &request).map_err(|e| {
        format!(
            "cannot reach the daemon at {} ({e}); is `systemctl --user status lumend` running?",
            path.display()
        )
    })?;
    if !response.ok {
        return Err(response
            .error
            .unwrap_or_else(|| "request failed".into())
            .into());
    }
    if raw {
        println!("{}", serde_json::to_string_pretty(&response.data)?);
    } else {
        print(&response.data);
    }
    Ok(())
}

fn num(v: &Value, digits: usize) -> String {
    v.as_f64()
        .map_or_else(|| "unknown".into(), |n| format!("{n:.digits$}"))
}

fn print_status(d: &Value) {
    println!("mode        {}", d["mode"].as_str().unwrap_or("?"));
    println!("device      {}", d["device"].as_str().unwrap_or("?"));
    println!("level       {} of {}", d["level"], d["max_level"]);
    if !d["target_level"].is_null() {
        println!(
            "target      {} (models disagree by ±{} p)",
            d["target_level"],
            num(&d["uncertainty"], 3)
        );
    }
    println!(
        "learned     {} corrections total, {} this session, {} stored samples",
        d["corrections_total"], d["corrections_this_session"], d["samples"]
    );
    if let Some(weights) = d["weights"].as_object() {
        let parts: Vec<String> = weights
            .iter()
            .map(|(k, v)| format!("{k} {:.0}%", v.as_f64().unwrap_or(0.0) * 100.0))
            .collect();
        println!("weights     {}", parts.join(", "));
    }
}

fn print_why(d: &Value) {
    if d["ready"] != Value::Bool(true) {
        println!("no prediction yet, try again in a second");
        return;
    }
    let s = &d["signals"];
    println!("target level {}", d["target_level"]);
    println!();
    println!(
        "room light   ~{} lx ({})",
        num(&s["room_lux"], 0),
        s["irradiance_source"].as_str().unwrap_or("?")
    );
    println!(
        "sky          {} W/m² of {} clear-sky, sun at {}°",
        num(&s["ghi"], 0),
        num(&s["clear_sky_ghi"], 0),
        num(&s["sun_elevation"], 1)
    );
    println!("screen luma  {}", num(&s["screen_luma"], 2));
    println!(
        "app          {}{}{}",
        s["app"].as_str().unwrap_or("none"),
        if s["fullscreen"] == Value::Bool(true) {
            ", fullscreen"
        } else {
            ""
        },
        if s["video_playing"] == Value::Bool(true) {
            ", video playing"
        } else {
            ""
        }
    );
    if !s["night_light_kelvin"].is_null() {
        println!("night light  {} K", s["night_light_kelvin"]);
    }
    println!();
    if let Some(experts) = d["experts"].as_array() {
        for e in experts {
            println!(
                "{:<10} level {:>4}  weight {:>3.0}%",
                e["name"].as_str().unwrap_or("?"),
                e["level"],
                e["weight"].as_f64().unwrap_or(0.0) * 100.0
            );
        }
    }
    let offset = d["short_term_offset"].as_f64().unwrap_or(0.0);
    if offset.abs() > 0.005 {
        println!("recent correction still shifts the target by {offset:+.3} p");
    }
    if let Some(drivers) = d["drivers"].as_array().filter(|a| !a.is_empty()) {
        println!();
        println!("what the linear model learned matters here:");
        for dr in drivers {
            println!(
                "  {:<22} {:+.3} p",
                dr["feature"].as_str().unwrap_or("?"),
                dr["shift"].as_f64().unwrap_or(0.0)
            );
        }
    }
}
