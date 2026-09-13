pub fn cholesky(a: &[f64], n: usize) -> Option<Vec<f64>> {
    let mut l = vec![0.0; n * n];
    for i in 0..n {
        for j in 0..=i {
            let dot: f64 = (0..j).map(|k| l[i * n + k] * l[j * n + k]).sum();
            if i == j {
                let d = a[i * n + i] - dot;
                if d <= 0.0 || !d.is_finite() {
                    return None;
                }
                l[i * n + i] = d.sqrt();
            } else {
                l[i * n + j] = (a[i * n + j] - dot) / l[j * n + j];
            }
        }
    }
    Some(l)
}

pub fn spd_inverse(a: &[f64], n: usize) -> Option<Vec<f64>> {
    let l = cholesky(a, n)?;
    let mut l_inv = vec![0.0; n * n];
    for col in 0..n {
        for row in col..n {
            let rhs = if row == col { 1.0 } else { 0.0 };
            let dot: f64 = (col..row)
                .map(|k| l[row * n + k] * l_inv[k * n + col])
                .sum();
            l_inv[row * n + col] = (rhs - dot) / l[row * n + row];
        }
    }
    let mut inv = vec![0.0; n * n];
    for i in 0..n {
        for j in 0..=i {
            let v: f64 = (i..n).map(|k| l_inv[k * n + i] * l_inv[k * n + j]).sum();
            inv[i * n + j] = v;
            inv[j * n + i] = v;
        }
    }
    Some(inv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_of_known_matrix() {
        let a = [4.0, 2.0, 0.6, 2.0, 5.0, 1.0, 0.6, 1.0, 3.0];
        let inv = spd_inverse(&a, 3).unwrap();
        for i in 0..3 {
            for j in 0..3 {
                let v: f64 = (0..3).map(|k| a[i * 3 + k] * inv[k * 3 + j]).sum();
                let expected = if i == j { 1.0 } else { 0.0 };
                assert!((v - expected).abs() < 1e-12, "({i},{j}) = {v}");
            }
        }
    }

    #[test]
    fn rejects_indefinite() {
        assert!(cholesky(&[1.0, 2.0, 2.0, 1.0], 2).is_none());
    }
}
