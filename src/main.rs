fn main() {
    let config = lumend::config::Config::load(&lumend::config::Config::default_path());
    println!("{config:?}");
}
