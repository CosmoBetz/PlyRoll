//! Roll inventory: the rolls in stock, with batch, remaining length and expiry.
//!
//! Kept in its own TOML file (`rolls.toml`, next to the material library) in
//! the same style as the library, so it can be edited by hand and shared. A
//! roll names its material by the library name. A job picks a roll by id; the
//! length left is always read from the inventory, never from a copy.
//!
//! Booking a nest on a roll takes its used length off the roll and records the
//! use, keyed by the result (fingerprint and seed), so the same nest cannot be
//! booked twice and a booking can be taken back.

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Roll {
    /// The roll's label, unique in the inventory.
    pub id: String,
    /// Material name in the library.
    pub material: String,
    /// Batch or lot number from the supplier.
    #[serde(default)]
    pub batch: String,
    /// Length left on the roll, m.
    pub remaining_m: f64,
    /// Use-by date, YYYY-MM-DD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// Nests booked on this roll, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub usage: Vec<Usage>,
}

/// One nest cut from a roll.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Usage {
    pub job: String,
    /// When it was booked, "YYYY-MM-DD HH:MM UTC".
    pub date: String,
    pub length_m: f64,
    /// Which nest: see `result_key`.
    pub result: String,
}

/// Identifies one nest result: the same plies, material and seed.
pub fn result_key(fingerprint: &str, seed: u64) -> String {
    format!("{fingerprint}:{seed}")
}

impl Roll {
    /// Expired on `today` (YYYY-MM-DD)?
    pub fn expired(&self, today: &str) -> bool {
        self.expires.as_deref().is_some_and(|e| e < today)
    }

    pub fn booking(&self, key: &str) -> Option<&Usage> {
        self.usage.iter().find(|u| u.result == key)
    }
}

/// Is `s` a real date written YYYY-MM-DD?
fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let (Ok(y), Ok(m), Ok(d)) = (s[0..4].parse::<u32>(), s[5..7].parse::<u32>(), s[8..10].parse::<u32>()) else {
        return false;
    };
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    (1..=12).contains(&m) && d >= 1 && d <= days[m as usize - 1]
}

pub fn validate_roll(r: &Roll) -> Result<()> {
    if r.id.trim().is_empty() {
        bail!("a roll needs an id");
    }
    let ctx = |msg: &str| anyhow!("roll '{}': {msg}", r.id);
    if r.material.trim().is_empty() {
        return Err(ctx("it needs a material"));
    }
    if !r.remaining_m.is_finite() || r.remaining_m < 0.0 || r.remaining_m > 100_000.0 {
        return Err(ctx("the remaining length must be between 0 and 100000 m"));
    }
    if let Some(e) = &r.expires
        && !valid_date(e)
    {
        return Err(ctx("the expiry date must be written YYYY-MM-DD"));
    }
    if r.usage.iter().any(|u| !u.length_m.is_finite() || u.length_m < 0.0) {
        return Err(ctx("a booked length is not valid"));
    }
    Ok(())
}

pub fn validate_rolls(rolls: &[Roll]) -> Result<()> {
    for (i, r) in rolls.iter().enumerate() {
        validate_roll(r)?;
        if rolls[..i].iter().any(|o| o.id.trim().eq_ignore_ascii_case(r.id.trim())) {
            bail!("two rolls have the id '{}'", r.id);
        }
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct RollFile {
    #[serde(default)]
    roll: Vec<Roll>,
}

pub fn load_rolls(path: &Path) -> Result<Vec<Roll>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let f: RollFile = toml::from_str(&text).with_context(|| format!("invalid roll inventory {}", path.display()))?;
    validate_rolls(&f.roll)?;
    Ok(f.roll)
}

/// Writes the inventory in the format `load_rolls` reads (via a temporary
/// file, so a failed write never leaves half a file).
pub fn save_rolls(path: &Path, rolls: &[Roll]) -> Result<()> {
    validate_rolls(rolls)?;
    let text = format!(
        "# PlyRoll roll inventory. remaining_m in metres, expires as YYYY-MM-DD.\n\n{}",
        toml::to_string_pretty(&RollFile { roll: rolls.to_vec() })?
    );
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).with_context(|| format!("cannot write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("cannot write {}", path.display()))?;
    Ok(())
}

/// Lengths are kept to 0.1 mm, so the file shows 0.7223, not 0.7223198878433621.
fn tidy(m: f64) -> f64 {
    (m * 10_000.0).round() / 10_000.0
}

/// Takes a nest's length off a roll and records it. Refused if this nest is
/// already booked on any roll, or the roll has less left than the nest needs.
pub fn book(rolls: &mut [Roll], id: &str, usage: Usage) -> Result<()> {
    let usage = Usage { length_m: tidy(usage.length_m), ..usage };
    if let Some(r) = rolls.iter().find(|r| r.booking(&usage.result).is_some()) {
        bail!("this nest is already booked on roll '{}'", r.id);
    }
    let r = rolls.iter_mut().find(|r| r.id == id).ok_or_else(|| anyhow!("roll '{id}' is not in the inventory"))?;
    if usage.length_m > r.remaining_m + 1e-9 {
        bail!(
            "roll '{id}' has {:.2} m left, the nest needs {:.2} m. Correct the roll's remaining length first if it is wrong.",
            r.remaining_m,
            usage.length_m
        );
    }
    r.remaining_m = tidy((r.remaining_m - usage.length_m).max(0.0));
    r.usage.push(usage);
    Ok(())
}

/// Takes a booking back: the length goes back on the roll.
pub fn unbook(rolls: &mut [Roll], id: &str, key: &str) -> Result<Usage> {
    let r = rolls.iter_mut().find(|r| r.id == id).ok_or_else(|| anyhow!("roll '{id}' is not in the inventory"))?;
    let i = r.usage.iter().position(|u| u.result == key).ok_or_else(|| anyhow!("this nest is not booked on roll '{id}'"))?;
    let u = r.usage.remove(i);
    r.remaining_m = tidy(r.remaining_m + u.length_m);
    Ok(u)
}

/// The roll as printed on the report.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RollInfo {
    pub id: String,
    pub batch: String,
    pub expires: Option<String>,
    /// Length on the roll before this job, m (a booking of this nest is added back).
    pub before_m: f64,
    pub expired: bool,
}

impl RollInfo {
    /// The report's view of `roll` for the nest with `key`, on `today`.
    pub fn of(roll: &Roll, key: &str, today: &str) -> Self {
        let booked = roll.booking(key).map_or(0.0, |u| u.length_m);
        RollInfo { id: roll.id.clone(), batch: roll.batch.clone(), expires: roll.expires.clone(), before_m: roll.remaining_m + booked, expired: roll.expired(today) }
    }
}
