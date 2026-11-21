//! Fibre-orientation rules: which placement rotations keep the fibres
//! (UD) or the warp (twill) along the roll.
//!
//! Roll convention: the roll runs along +x. A ply whose fibre direction in the
//! drawing is θ ends up with fibres at θ + r after rotating it by r, so a
//! placement is valid when θ + r is a multiple of the material's period
//! (180° in general, 90° for twill with interchangeable warp/weft).
//! Flipping is never allowed.

use crate::geom::norm_deg;
use crate::model::{Material, MaterialKind};

/// Period of the allowed fibre directions on the roll, degrees.
pub fn period(material: &Material, cosmetic: bool) -> f64 {
    match material.kind {
        MaterialKind::Twill if material.warp_weft_interchangeable && !cosmetic => 90.0,
        _ => 180.0,
    }
}

/// Allowed placement rotations (degrees, normalised to [0, 360), sorted, unique)
/// for a ply whose fibre direction in the drawing is `fibre_deg`.
pub fn allowed_rotations(fibre_deg: f64, material: &Material, cosmetic: bool) -> Vec<f64> {
    let per = period(material, cosmetic);
    let base = -fibre_deg;
    let mut offsets = vec![0.0];
    if material.angle_tolerance > 0.0 && material.tolerance_step > 0.0 {
        let n = (material.angle_tolerance / material.tolerance_step + 1e-9).floor() as i32;
        for j in 1..=n {
            let o = j as f64 * material.tolerance_step;
            offsets.push(o);
            offsets.push(-o);
        }
    }
    let k_max = (360.0 / per).round() as i32;
    let mut out: Vec<f64> = (0..k_max)
        .flat_map(|k| offsets.iter().map(move |o| round6(norm_deg(base + k as f64 * per + o))))
        .collect();
    out.sort_by(f64::total_cmp);
    out.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    // 0 and 359.9999999 are the same angle.
    if out.len() > 1 && (out[out.len() - 1] - 360.0).abs() < 1e-6 {
        out.pop();
    }
    out
}

fn round6(a: f64) -> f64 {
    let r = (a * 1e6).round() / 1e6;
    if r >= 360.0 { r - 360.0 } else { r }
}

/// Deviation of the placed fibre direction from the nearest allowed roll
/// direction, degrees.
pub fn fibre_deviation(fibre_deg: f64, rotation_deg: f64, material: &Material, cosmetic: bool) -> f64 {
    let per = period(material, cosmetic);
    let a = (fibre_deg + rotation_deg).rem_euclid(per);
    a.min(per - a)
}

pub fn fibre_ok(fibre_deg: f64, rotation_deg: f64, material: &Material, cosmetic: bool) -> bool {
    // Small slack for f32 rounding inside the solver.
    fibre_deviation(fibre_deg, rotation_deg, material, cosmetic) <= material.angle_tolerance + 1e-3
}

/// Key used to merge variants of one shape that have identical rotation sets.
pub fn rotation_key(rots: &[f64]) -> Vec<i64> {
    rots.iter().map(|r| (r * 1000.0).round() as i64).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mat(kind: MaterialKind, swap: bool, tol: f64) -> Material {
        Material {
            name: "m".into(),
            kind,
            roll_width: 1000.0,
            edge_trim: 0.0,
            lead_trim: 0.0,
            spacing: 2.0,
            angle_tolerance: tol,
            tolerance_step: 0.5,
            warp_weft_interchangeable: swap,
            cost_per_m2: None,
        }
    }

    #[test]
    fn ud_has_two_rotations() {
        let m = mat(MaterialKind::Ud, false, 0.0);
        assert_eq!(allowed_rotations(0.0, &m, false), vec![0.0, 180.0]);
        assert_eq!(allowed_rotations(45.0, &m, false), vec![135.0, 315.0]);
        assert_eq!(allowed_rotations(-45.0, &m, false), vec![45.0, 225.0]);
    }

    #[test]
    fn plus_and_minus_45_differ_for_ud_and_default_twill() {
        for m in [mat(MaterialKind::Ud, false, 0.0), mat(MaterialKind::Twill, false, 0.0)] {
            let a = rotation_key(&allowed_rotations(45.0, &m, false));
            let b = rotation_key(&allowed_rotations(-45.0, &m, false));
            assert_ne!(a, b);
        }
    }

    #[test]
    fn plus_and_minus_45_equal_for_interchangeable_twill() {
        let m = mat(MaterialKind::Twill, true, 0.0);
        let a = allowed_rotations(45.0, &m, false);
        let b = allowed_rotations(-45.0, &m, false);
        assert_eq!(a.len(), 4);
        assert_eq!(rotation_key(&a), rotation_key(&b));
        // ...unless the ply is cosmetic.
        assert_ne!(rotation_key(&allowed_rotations(45.0, &m, true)), rotation_key(&allowed_rotations(-45.0, &m, true)));
    }

    #[test]
    fn every_allowed_rotation_aligns_fibres() {
        let m = mat(MaterialKind::Ud, false, 1.0);
        for fibre in [-90.0, -45.0, 0.0, 17.3, 30.0, 45.0, 90.0, 123.4] {
            for r in allowed_rotations(fibre, &m, false) {
                assert!(fibre_ok(fibre, r, &m, false), "fibre {fibre} rot {r}");
            }
        }
        // A 90° error is caught.
        assert!(!fibre_ok(0.0, 90.0, &m, false));
        assert!(!fibre_ok(45.0, 0.0, &m, false));
    }

    #[test]
    fn tolerance_adds_angles() {
        let m = mat(MaterialKind::Ud, false, 1.0);
        let r = allowed_rotations(0.0, &m, false);
        assert_eq!(r, vec![0.0, 0.5, 1.0, 179.0, 179.5, 180.0, 180.5, 181.0, 359.0, 359.5]);
    }
}
