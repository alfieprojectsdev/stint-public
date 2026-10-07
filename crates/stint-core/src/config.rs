//! Single source of truth for rate, categories, clients, and paths.
//!
//! Port of `stintcore/config.py`. The constants here must agree with the
//! Python module and the Bash `stint.sh` script; the parity suite in `tests/`
//! (pytest, driven through the `stint parity` subcommand) is what keeps the
//! three in step.

use std::path::{Path, PathBuf};

use rust_decimal::Decimal;

/// Billable rate, $/hr. One place, one value.
pub const RATE: Decimal = Decimal::from_parts(1600, 0, 0, false, 2); // 16.00

/// Rate in whole cents, for integer-only arithmetic on the engine path.
pub const RATE_CENTS: i64 = 1600;

/// Billing granularity: measured stopwatch time is rounded UP to the next
/// quarter-hour mark (see `engine::round_up_to_quarter`). Manual entries are
/// exempt.
pub const QUARTER_HOUR_SECS: i64 = 900;

/// Invoice line-item rounding granularity, in hours.
pub const INVOICE_ROUNDING: Decimal = Decimal::from_parts(25, 0, 0, false, 2); // 0.25

/// Monthly ledger filenames are `<CSV_PREFIX>YYYY-MM.csv`. The tool was once
/// named after a client, so months written before the rename are
/// `<LEGACY_CSV_PREFIX>YYYY-MM.csv`; every reader accepts both and a month that
/// already exists under the legacy name keeps it. Nothing needs migrating.
pub const CSV_PREFIX: &str = "stint-";
pub const LEGACY_CSV_PREFIX: &str = "savd-";

/// Split `<prefix>YYYY-MM.csv` into (year, month) for either prefix.
pub fn parse_month_filename(name: &str) -> Option<(i32, u32)> {
    let stem = name
        .strip_prefix(CSV_PREFIX)
        .or_else(|| name.strip_prefix(LEGACY_CSV_PREFIX))?
        .strip_suffix(".csv")?;
    let (y, m) = stem.split_once('-')?;
    Some((y.parse().ok()?, m.parse().ok()?))
}

pub const CSV_HEADER: &str = "date,start_time,end_time,duration_hrs,category,description";

/// Ordered so help text / dropdowns are stable.
pub const CATEGORIES: [&str; 9] = [
    "pr", "async", "standup", "devops", "research", "dev", "admin", "docs", "planning",
];

pub const DEFAULT_CATEGORY: &str = "dev";

pub fn is_category(token: &str) -> bool {
    CATEGORIES.contains(&token)
}

/// Human label used on invoices (was `CAT_LABELS` in legacy-consolidate.py).
pub fn category_label(category: &str) -> &'static str {
    match category {
        "pr" => "Pull Request Reviews",
        "async" => "Async Communication",
        "standup" => "Standups",
        "devops" => "DevOps",
        "research" => "Research",
        "dev" => "Development",
        "admin" => "Administrative",
        "docs" => "Documentation",
        "planning" => "Planning",
        _ => "Other",
    }
}

/// A billable client.
///
/// `html_capable` gates automated HTML generation: a flat-rate USD client
/// renders through the invoice module. A client whose invoices need another
/// currency, withholding tax or payment block stays hand-built: the tool only
/// reserves the next number and refuses to render HTML.
///
/// Real client identities are personal data and live OUTSIDE the tracked
/// tree, in `<log_dir>/clients.json` (git-ignored; same file the Python core
/// reads). Without one, [`default_clients`] keeps every front-end working.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Client {
    pub key: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub counter_file: String,
    #[serde(default = "default_seed")]
    pub counter_seed: u32,
    #[serde(default = "default_currency")]
    pub currency: String,
    #[serde(default)]
    pub legal_name: String,
    #[serde(default)]
    pub address: Vec<String>,
    #[serde(default)]
    pub project_title: String,
    #[serde(default = "default_true")]
    pub html_capable: bool,
}

fn default_seed() -> u32 {
    1001
}
fn default_currency() -> String {
    "USD".into()
}
fn default_true() -> bool {
    true
}

impl Client {
    fn normalized(mut self) -> Self {
        if self.display_name.is_empty() {
            self.display_name = self.key.clone();
        }
        if self.counter_file.is_empty() {
            self.counter_file = format!(".invoice-counter-{}", self.key);
        }
        self
    }
}

/// Placeholders used when `clients.json` is absent (tests, demo, a fresh clone).
pub fn default_clients() -> Vec<Client> {
    vec![
        Client {
            key: "acme".into(),
            display_name: "Acme Corp".into(),
            counter_file: ".invoice-counter".into(),
            counter_seed: 1001,
            currency: "USD".into(),
            legal_name: "Acme Corp LLC".into(),
            address: vec!["123 Example St".into(), "Springfield, ST 00000".into(), "USA".into()],
            project_title: "Software Development".into(),
            html_capable: true,
        },
        Client {
            key: "globex".into(),
            display_name: "Globex Ltd".into(),
            counter_file: ".invoice-counter-globex".into(),
            counter_seed: 2001,
            currency: "PHP".into(),
            legal_name: "Globex Ltd".into(),
            address: vec!["1 Example Ave".into(), "Example City 1000".into(), "PH".into()],
            project_title: String::new(),
            html_capable: false, // hand-built invoice: number reserved only
        },
    ]
}

#[derive(serde::Deserialize)]
struct ClientsFile {
    #[serde(default)]
    default_client: Option<String>,
    #[serde(default)]
    clients: Vec<Client>,
}

/// Load `<dir>/clients.json`. Missing file -> placeholders. A malformed file
/// is an error so a typo can't silently invoice the wrong client.
pub fn load_clients(dir: &Path) -> std::io::Result<(Vec<Client>, String)> {
    let path = dir.join("clients.json");
    if !path.exists() {
        let d = default_clients();
        let key = d[0].key.clone();
        return Ok((d, key));
    }
    let text = std::fs::read_to_string(&path)?;
    let parsed: ClientsFile = serde_json::from_str(&text)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{}: {e}", path.display())))?;
    if parsed.clients.is_empty() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{}: no clients defined", path.display())));
    }
    let clients: Vec<Client> = parsed.clients.into_iter().map(Client::normalized).collect();
    let default = parsed.default_client.unwrap_or_else(|| clients[0].key.clone());
    if !clients.iter().any(|c| c.key == default) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{}: default_client {default:?} is not a defined client", path.display()),
        ));
    }
    Ok((clients, default))
}

/// Where the ledger lives. Resolved once at startup and threaded through the
/// store explicitly (no process-global), so a GUI can open a different home
/// without restarting and tests can point at a temp dir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home {
    /// Directory holding `stint-YYYY-MM.csv`, `.timers/`, `temp/`, counters.
    pub log_dir: PathBuf,
    /// True when pointed at the synthetic `samples/` dataset.
    pub demo: bool,
    /// Clients from `<log_dir>/clients.json`, or the placeholders.
    pub clients: Vec<Client>,
    pub default_client: String,
}

impl Home {
    /// A home over `log_dir`. A malformed `clients.json` is reported on
    /// stderr and the placeholders are used, so a typo in the file can't take
    /// the timer verbs down with it; the invoice paths surface it again.
    pub fn new(log_dir: impl Into<PathBuf>) -> Self {
        let log_dir = log_dir.into();
        let (clients, default_client) = match load_clients(&log_dir) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("stint: {e}; using placeholder clients");
                let d = default_clients();
                let k = d[0].key.clone();
                (d, k)
            }
        };
        Home { log_dir, demo: false, clients, default_client }
    }

    pub fn client(&self, key: &str) -> Option<&Client> {
        self.clients.iter().find(|c| c.key == key)
    }

    /// First client whose invoices are hand-built, if any.
    pub fn manual_client(&self) -> Option<&Client> {
        self.clients.iter().find(|c| !c.html_capable)
    }

    /// Resolution order (mirrors the Bash script so the two coexist on one
    /// ledger):
    ///   1. `explicit` (a `--home` flag),
    ///   2. `STINT_LOG_DIR` (the Bash script's own override),
    ///   3. `STINT_HOME`,
    ///   4. the folder remembered by the GUI's ledger picker
    ///      (`%APPDATA%\stint\home.txt` / `~/.config/stint/home.txt`),
    ///   5. `$HOME/repos/stint` (Bash default; a symlink to the repo on
    ///      the live machine).
    /// `STINT_DEMO=1` then redirects to `<log_dir>/samples`, exactly like
    /// `stintcore.config.DEMO`.
    pub fn resolve(explicit: Option<&Path>) -> Self {
        let base = explicit
            .map(Path::to_path_buf)
            .or_else(|| env_path("STINT_LOG_DIR"))
            .or_else(|| env_path("SAVD_LOG_DIR")) // pre-rename spelling
            .or_else(|| env_path("STINT_HOME"))
            .or_else(saved_home)
            .unwrap_or_else(default_ledger_dir);
        let demo = ["STINT_DEMO", "SAVD_DEMO"] // the latter is the pre-rename spelling
            .iter()
            .any(|k| std::env::var_os(k).is_some_and(|v| v == "1"));
        let log_dir = if demo { base.join("samples") } else { base };
        let mut home = Home::new(log_dir);
        home.demo = demo;
        home
    }

    pub fn timers_dir(&self) -> PathBuf {
        self.log_dir.join(".timers")
    }

    pub fn temp_dir(&self) -> PathBuf {
        self.log_dir.join("temp")
    }

    /// The month's CSV. A month already stored under the legacy prefix keeps
    /// that file; anything new gets `stint-YYYY-MM.csv`.
    pub fn csv_path(&self, year: i32, month: u32) -> PathBuf {
        let current = self.log_dir.join(format!("{CSV_PREFIX}{year:04}-{month:02}.csv"));
        if current.exists() {
            return current;
        }
        let legacy = self.log_dir.join(format!("{LEGACY_CSV_PREFIX}{year:04}-{month:02}.csv"));
        if legacy.exists() {
            return legacy;
        }
        current
    }

    pub fn counter_path(&self, client: &Client) -> PathBuf {
        self.log_dir.join(&client.counter_file)
    }

    /// True when this folder holds (or has held) a ledger: any monthly CSV
    /// or a `.timers/` dir. Used by the GUI to decide whether to ask.
    pub fn looks_like_ledger(&self) -> bool {
        if self.timers_dir().is_dir() {
            return true;
        }
        std::fs::read_dir(&self.log_dir)
            .map(|d| {
                d.flatten().any(|e| {
                    let n = e.file_name();
                    let n = n.to_string_lossy();
                    parse_month_filename(&n).is_some()
                })
            })
            .unwrap_or(false)
    }

    /// Remember `log_dir` as the default for future runs (step 4 above).
    pub fn save_as_default(&self) -> std::io::Result<PathBuf> {
        let path = saved_home_file();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, format!("{}\n", self.log_dir.display()))?;
        Ok(path)
    }
}

/// Per-user config file holding the remembered ledger folder.
pub fn saved_home_file() -> PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| home_dir().join("AppData").join("Roaming"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home_dir().join(".config"))
    };
    base.join("stint").join("home.txt")
}

fn saved_home() -> Option<PathBuf> {
    let text = std::fs::read_to_string(saved_home_file()).ok()?;
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    Some(PathBuf::from(line))
}

/// `~/repos/stint` (the repo is the ledger), or the pre-rename
/// `~/repos/savd-logs` when only that exists (a symlink on older setups).
fn default_ledger_dir() -> PathBuf {
    let repos = home_dir().join("repos");
    let current = repos.join("stint");
    let legacy = repos.join("savd-logs");
    if !current.exists() && legacy.exists() {
        legacy
    } else {
        current
    }
}

/// An env var that is set but empty counts as unset, like Bash `${VAR:-default}`.
fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_is_sixteen_dollars() {
        assert_eq!(RATE.to_string(), "16.00");
        assert_eq!(RATE_CENTS, 1600);
    }

    #[test]
    fn categories_match_python() {
        assert!(is_category("pr"));
        assert!(!is_category("PR"));
        assert_eq!(CATEGORIES.len(), 9);
        assert_eq!(category_label("pr"), "Pull Request Reviews");
    }

    #[test]
    fn placeholder_clients_are_disjoint_blocks() {
        let home = Home::new(std::env::temp_dir().join("stint-no-clients-file"));
        let main = home.client(&home.default_client).unwrap();
        let manual = home.manual_client().unwrap();
        assert!(main.html_capable);
        assert!(!manual.html_capable);
        assert_eq!(main.counter_seed / 1000, 1);
        assert_eq!(manual.counter_seed / 1000, 2);
    }

    #[test]
    fn clients_json_overrides_placeholders() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("clients.json"),
            r#"{"default_client":"zed","clients":[{"key":"zed","legal_name":"Zed Co","address":["1 Road"],"counter_seed":1500}]}"#,
        )
        .unwrap();
        let home = Home::new(dir.path());
        assert_eq!(home.default_client, "zed");
        let z = home.client("zed").unwrap();
        assert_eq!(z.display_name, "zed");
        assert_eq!(z.counter_file, ".invoice-counter-zed");
        assert_eq!(z.currency, "USD");
        assert!(z.html_capable);
        assert!(home.manual_client().is_none());
        // Malformed -> placeholders, not a panic.
        std::fs::write(dir.path().join("clients.json"), "{not json").unwrap();
        let home = Home::new(dir.path());
        assert_eq!(home.default_client, "acme");
    }

    #[test]
    fn empty_env_var_counts_as_unset() {
        // Not using the real env: exercise the helper directly.
        std::env::set_var("STINT_TEST_EMPTY", "");
        assert_eq!(env_path("STINT_TEST_EMPTY"), None);
        std::env::set_var("STINT_TEST_SET", "/x");
        assert_eq!(env_path("STINT_TEST_SET"), Some(PathBuf::from("/x")));
    }

    #[test]
    fn explicit_home_wins() {
        let h = Home::resolve(Some(Path::new("/tmp/x")));
        assert_eq!(h.log_dir, PathBuf::from("/tmp/x"));
        assert_eq!(h.csv_path(2026, 7).file_name().unwrap(), "stint-2026-07.csv");
    }
}
