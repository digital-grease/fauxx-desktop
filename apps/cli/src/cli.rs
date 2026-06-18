// fauxx-desktop: Fauxx Desktop Companion
// Copyright (C) 2026 Digital Grease
//
// This program is free software: you can redistribute it and/or modify it
// under the terms of the GNU Affero General Public License as published by the
// Free Software Foundation, either version 3 of the License, or (at your
// option) any later version.
//
// This program is distributed in the hope that it will be useful, but WITHOUT
// ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
// FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more
// details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

//! Command-line surface and store-config resolution.
//!
//! This module owns the clap-derive types and the single place that turns the
//! global store flags (`--db`, `--passphrase-file`, `--key-file`,
//! `--prompt-passphrase`) into a [`fauxx_core::Config`]. Keeping that
//! resolution here means every subcommand opens the core the same way, which is
//! the only I/O these commands do before delegating to the core (the thin
//! client rule: no business logic lives in the CLI).

use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use clap::{Args, Parser, Subcommand};
use fauxx_core::{Config, KeySource};

/// Top-level CLI definition for the `fauxx` binary.
#[derive(Parser, Debug)]
#[command(
    name = "fauxx",
    about = "Fauxx desktop companion (headless CLI), a thin client over fauxx-core",
    version,
    arg_required_else_help = true
)]
pub struct Cli {
    /// Global store-key options shared by every subcommand.
    #[command(flatten)]
    pub store: StoreOpts,

    /// The subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

/// Options that select where the encrypted store lives and how its key is
/// sourced. Shared by all subcommands via `#[command(flatten)]`.
///
/// Key-source resolution:
/// - No passphrase flags: the OS keystore (the desktop default).
/// - `--passphrase-file <PATH>`: read the passphrase from that file and unlock
///   an Argon2id-wrapped key file. `--passphrase-file -` reads the passphrase
///   from an interactive hidden prompt instead.
/// - `--prompt-passphrase`: force the interactive hidden prompt.
///
/// The key file path defaults to a `<db>.key` sibling of the database and can
/// be overridden with `--key-file <PATH>`.
#[derive(Args, Debug, Default)]
pub struct StoreOpts {
    /// Override the encrypted store path (default: the OS data directory).
    #[arg(long, value_name = "PATH", global = true, env = "FAUXX_DB")]
    pub db: Option<PathBuf>,

    /// Read the store passphrase from this file (use `-` for an interactive
    /// hidden prompt). Selects the headless encrypted-key-file key source.
    #[arg(
        long,
        value_name = "PATH",
        global = true,
        env = "FAUXX_PASSPHRASE_FILE"
    )]
    pub passphrase_file: Option<PathBuf>,

    /// Prompt for the store passphrase on the terminal (hidden input). Selects
    /// the headless encrypted-key-file key source.
    #[arg(long, global = true, conflicts_with = "passphrase_file")]
    pub prompt_passphrase: bool,

    /// Override the Argon2id-wrapped key file path (default: `<db>.key`). Only
    /// meaningful with a passphrase key source.
    #[arg(long, value_name = "PATH", global = true, env = "FAUXX_KEY_FILE")]
    pub key_file: Option<PathBuf>,
}

impl StoreOpts {
    /// Resolve these options into a [`Config`] the core can open.
    ///
    /// Reads the passphrase (file or interactive prompt) when a passphrase key
    /// source is selected; otherwise defaults to the OS keystore. Does no other
    /// I/O. Returns a usage-style error (mapped to exit code 2 by the caller)
    /// when the flags do not name a usable passphrase.
    pub fn to_config(&self) -> anyhow::Result<Config> {
        let mut config = Config::new();
        if let Some(db) = &self.db {
            config = config.with_path(db.clone());
        }

        // `None` means no passphrase flags: keep the default OS keystore source.
        if let Some(source) = self.resolve_key_source()? {
            config = config.with_key_source(source);
        }
        Ok(config)
    }

    /// Resolve the passphrase key source, or `None` to keep the OS keystore.
    fn resolve_key_source(&self) -> anyhow::Result<Option<KeySource>> {
        let passphrase = if self.prompt_passphrase {
            prompt_passphrase()?
        } else if let Some(file) = &self.passphrase_file {
            if file.as_os_str() == "-" {
                prompt_passphrase()?
            } else {
                read_passphrase_file(file)?
            }
        } else {
            // OS keystore path: no passphrase, no key file.
            return Ok(None);
        };

        let key_path = self.resolve_key_path()?;
        Ok(Some(KeySource::EncryptedFile {
            path: key_path,
            passphrase,
        }))
    }

    /// Resolve the key file path: `--key-file` if given, else `<db>.key` next to
    /// the database (using the explicit `--db` path, or the OS default path).
    fn resolve_key_path(&self) -> anyhow::Result<PathBuf> {
        if let Some(key_file) = &self.key_file {
            return Ok(key_file.clone());
        }
        let db_path = match &self.db {
            Some(db) => db.clone(),
            None => fauxx_core::store::EncryptedStore::default_path().context(
                "could not determine the default store path to derive the key file path",
            )?,
        };
        Ok(sibling_key_path(&db_path))
    }
}

/// Derive the `<db>.key` sibling path for a database path. Appends `.key` to
/// the file name so `fauxx.db` becomes `fauxx.db.key`.
fn sibling_key_path(db_path: &Path) -> PathBuf {
    let mut name = db_path.file_name().unwrap_or_default().to_os_string();
    name.push(".key");
    db_path.with_file_name(name)
}

/// Read a passphrase from a file, trimming a single trailing newline so a file
/// written by `echo`/an editor unlocks the same key as the raw bytes.
fn read_passphrase_file(path: &Path) -> anyhow::Result<String> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading passphrase file {}", path.display()))?;
    let trimmed = raw.strip_suffix('\n').unwrap_or(&raw);
    let trimmed = trimmed.strip_suffix('\r').unwrap_or(trimmed);
    if trimmed.is_empty() {
        bail!("passphrase file {} is empty", path.display());
    }
    Ok(trimmed.to_string())
}

/// Prompt for a passphrase on the terminal with hidden (non-echoed) input.
fn prompt_passphrase() -> anyhow::Result<String> {
    let passphrase =
        rpassword::prompt_password("store passphrase: ").context("reading passphrase prompt")?;
    if passphrase.is_empty() {
        bail!("empty passphrase");
    }
    Ok(passphrase)
}

/// The `fauxx` subcommands. The C0 #4 foundation (`run`, `status`, `persona`)
/// plus the C1 cross-device surface (`pair`, `peers`, `unpair`, `mode`,
/// `schedule`), which makes the sync/coordination API reachable headlessly.
/// Richer agent control lands in later milestones (C8 #35 serve mode).
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Run the headless agent (open the core, then block until Ctrl-C).
    Run,

    /// Run a one-off decoy SEARCH session for a persona (C6 H1): generate
    /// persona-aligned, safety-gated queries and dispatch them to search engines
    /// through the persona's isolated decoy browser. Standalone search-engine
    /// poisoning for a phone-less / homelab deployment. Requires a system Chromium.
    Search {
        /// The persona id to search as.
        persona_id: String,

        /// The decoy-profile id (a dedicated, isolated browser-profile dir).
        /// Defaults to `search-<persona_id>`.
        #[arg(long)]
        decoy_id: Option<String>,

        /// Emit the session outcome as JSON instead of a summary.
        #[arg(long)]
        json: bool,
    },

    /// Print headless core status.
    Status {
        /// Emit the status as JSON instead of a human-readable line.
        #[arg(long)]
        json: bool,
    },

    /// Inspect and manage synthetic personas.
    Persona {
        #[command(subcommand)]
        command: PersonaCommand,
    },

    /// Cross-device pairing (show this device's QR, or add a scanned peer).
    Pair {
        #[command(subcommand)]
        command: PairCommand,
    },

    /// List paired (or, with `--discovered`, mDNS-discovered) peers.
    Peers {
        /// List mDNS-discovered (untrusted) peers instead of paired ones.
        #[arg(long)]
        discovered: bool,

        /// Emit the peer list as JSON instead of a summary table.
        #[arg(long)]
        json: bool,
    },

    /// Remove a paired peer by its base64url public key.
    Unpair {
        /// The peer's base64url public key (as shown by `fauxx peers --json`).
        public_key: String,
    },

    /// Show or set the household coordination mode.
    Mode {
        #[command(subcommand)]
        command: Option<ModeCommand>,
    },

    /// Preview the household action schedule over the paired devices.
    Schedule {
        /// Seed for the deterministic plan (same seed yields the same plan).
        #[arg(long, default_value_t = 0)]
        seed: u64,

        /// Print at most this many scheduled actions (the plan summary always
        /// reflects the full plan).
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },

    /// Data-broker opt-out registry and submission tracking (C3 #15).
    Broker {
        #[command(subcommand)]
        command: BrokerCommand,
    },

    /// GDPR/CCPA Data Subject Access Request letters (C3 #16).
    Dsar {
        #[command(subcommand)]
        command: DsarCommand,
    },

    /// Per-persona email alias management (C3 #17).
    Alias {
        #[command(subcommand)]
        command: AliasCommand,
    },

    /// Account-anchor identity-linkage inventory and analysis (C3 #19).
    Anchor {
        #[command(subcommand)]
        command: AnchorCommand,
    },

    /// Per-site Global Privacy Control honoring observations (C3 #18).
    Gpc {
        #[command(subcommand)]
        command: GpcCommand,
    },

    /// Export an efficacy snapshot to CSV/JSON/PDF (C4 #23).
    Export(ExportArgs),

    /// Control-profile A/B shadow profiles and cohort comparison (C4 #21).
    Ab {
        #[command(subcommand)]
        command: AbCommand,
    },

    /// Print the per-platform KL-divergence drift for a persona (C4 #20).
    Drift(DriftArgs),

    /// Signed persona-pack import/export and the installed-pack library (C5 #27).
    Pack {
        #[command(subcommand)]
        command: PackCommand,
    },

    /// Run a generation pass producing signed artifacts (C6 #28).
    Generate(GenerateArgs),

    /// Mint N coherent PUMS personas into a signed pack (C6 #29).
    Mint(MintArgs),

    /// Per-persona network egress (C7 #30).
    Egress {
        #[command(subcommand)]
        command: EgressCommand,
    },

    /// Per-persona DNS strategy (C7 #31).
    Dns {
        #[command(subcommand)]
        command: DnsCommand,
    },

    /// Goal-driven campaigns: the closed loop (C8 #33).
    Campaign {
        #[command(subcommand)]
        command: CampaignCommand,
    },

    /// Long-running headless homelab mode driven by a config file (C8 #35).
    Serve(ServeArgs),

    /// Browser native-messaging host: bridge the C2 #14 WebExtension to the core
    /// over stdin/stdout (R4). Launched by the browser, not run interactively;
    /// see `extension/native-host/README.md`.
    #[command(name = "native-host")]
    NativeHost,

    /// Persisted debug logs: show where they live, export a scrubbed copy to
    /// attach to a bug report, or clear them.
    Logs {
        #[command(subcommand)]
        command: LogsCommand,
    },
}

/// The `fauxx logs ...` subcommands (the bug-report path).
#[derive(Subcommand, Debug)]
pub enum LogsCommand {
    /// Print the directory where the debug logs are written.
    Path,
    /// Export a SCRUBBED copy of the debug logs to one shareable file (paths,
    /// IPs, emails, keys, ids, and persona names redacted) for a bug report.
    Export {
        /// Output file (default: ./fauxx-debug-log.txt).
        #[arg(long, value_name = "PATH")]
        out: Option<PathBuf>,
    },
    /// Delete all persisted debug log files.
    Clear,
}

/// The `fauxx broker ...` subcommands (C3 #15).
#[derive(Subcommand, Debug)]
pub enum BrokerCommand {
    /// List the bundled data-broker opt-out registry.
    List {
        /// Emit the registry as JSON instead of a summary table.
        #[arg(long)]
        json: bool,
    },
    /// Generate (without recording) a filled opt-out request to review.
    Generate {
        /// The broker id (as shown by `fauxx broker list`).
        broker_id: String,
        /// The persona id this request is for.
        persona_id: String,
        /// Emit the filled request as JSON instead of a summary.
        #[arg(long)]
        json: bool,
    },
    /// Generate AND record a drafted opt-out submission.
    Record {
        /// The broker id.
        broker_id: String,
        /// The persona id this submission is for.
        persona_id: String,
    },
    /// List recorded broker submissions (optionally scoped to a persona).
    Submissions {
        /// Scope the list to one persona id.
        #[arg(long)]
        persona: Option<String>,
        /// Emit the submissions as JSON.
        #[arg(long)]
        json: bool,
    },
    /// List submissions whose broker deadline is due or overdue as of now.
    DueSoon {
        /// Emit the due list as JSON.
        #[arg(long)]
        json: bool,
    },
}

/// The `fauxx dsar ...` subcommands (C3 #16).
#[derive(Subcommand, Debug)]
pub enum DsarCommand {
    /// Generate (without recording) a DSAR letter to review.
    Generate(DsarRequestArgs),
    /// Generate AND record a drafted DSAR request.
    Record(DsarRequestArgs),
    /// List recorded DSAR requests (optionally scoped to a persona).
    List {
        /// Scope the list to one persona id.
        #[arg(long)]
        persona: Option<String>,
        /// Emit the requests as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Export (render) a recorded request's letter text for manual sending.
    Export {
        /// The request id (as shown by `fauxx dsar list`).
        request_id: String,
        /// The real legal name the letter is signed with (never persisted).
        #[arg(long)]
        name: String,
        /// A contact line (email/postal) the controller should reply to.
        #[arg(long, default_value = "")]
        contact: String,
    },
    /// List DSAR requests that are overdue as of now.
    Overdue {
        /// Emit the overdue list as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Mark a recorded request as SENT now, which starts its statutory deadline
    /// clock (C3 #16: until a request is sent, no deadline is tracked).
    Sent {
        /// The request id (as shown by `fauxx dsar list`).
        request_id: String,
    },
}

/// Shared arguments for `dsar generate` / `dsar record`.
#[derive(Args, Debug)]
pub struct DsarRequestArgs {
    /// The request kind.
    #[arg(value_enum)]
    pub kind: DsarKindArg,
    /// The persona id this request is filed on behalf of.
    pub persona_id: String,
    /// Target a known broker id from the registry as the controller.
    #[arg(long, conflicts_with = "controller_name")]
    pub broker: Option<String>,
    /// Name an arbitrary controller (instead of `--broker`).
    #[arg(long = "controller-name", conflicts_with = "broker")]
    pub controller_name: Option<String>,
    /// A contact line for an arbitrary controller.
    #[arg(long = "controller-contact", default_value = "")]
    pub controller_contact: String,
}

/// The DSAR request kinds accepted on the CLI.
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum DsarKindArg {
    /// GDPR right of access (Art. 15).
    GdprAccess,
    /// GDPR right to erasure (Art. 17).
    GdprDeletion,
    /// CCPA/CPRA right to know.
    CcpaAccess,
    /// CCPA/CPRA right to delete.
    CcpaDeletion,
}

impl From<DsarKindArg> for fauxx_core::RequestKind {
    fn from(arg: DsarKindArg) -> Self {
        match arg {
            DsarKindArg::GdprAccess => fauxx_core::RequestKind::GdprAccess,
            DsarKindArg::GdprDeletion => fauxx_core::RequestKind::GdprDeletion,
            DsarKindArg::CcpaAccess => fauxx_core::RequestKind::CcpaAccess,
            DsarKindArg::CcpaDeletion => fauxx_core::RequestKind::CcpaDeletion,
        }
    }
}

/// The `fauxx alias ...` subcommands (C3 #17).
#[derive(Subcommand, Debug)]
pub enum AliasCommand {
    /// Mint a fresh plus-address alias for a persona/site.
    Mint {
        /// The persona id.
        persona_id: String,
        /// The site the alias fronts.
        site: String,
        /// The base address to derive the plus-address from (`local@domain`).
        #[arg(long)]
        base: String,
        /// Allow reusing an address that already fronts a different site.
        #[arg(long)]
        allow_reuse: bool,
    },
    /// Record a manually-created alias (e.g. an iCloud Hide-My-Email forward).
    Record {
        /// The persona id.
        persona_id: String,
        /// The site the alias fronts.
        site: String,
        /// The address to record.
        address: String,
        /// Allow reusing an address that already fronts a different site.
        #[arg(long)]
        allow_reuse: bool,
    },
    /// List email aliases (optionally scoped to a persona).
    List {
        /// Scope the list to one persona id.
        #[arg(long)]
        persona: Option<String>,
        /// Emit the aliases as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Revoke an alias by id (kept for the audit trail).
    Revoke {
        /// The alias id (as shown by `fauxx alias list`).
        alias_id: String,
    },
    /// Rotate an alias: revoke the old one and mint a fresh plus-address.
    Rotate {
        /// The alias id to rotate.
        alias_id: String,
        /// The base address to derive the fresh plus-address from.
        #[arg(long)]
        base: String,
    },
}

/// The `fauxx gpc ...` subcommands (C3 #18): inspect the per-site Global Privacy
/// Control honoring observations the decoy/extension recorded.
#[derive(Subcommand, Debug)]
pub enum GpcCommand {
    /// List every recorded per-site GPC honoring observation.
    List,
    /// Show the recorded GPC status for one site origin.
    Status {
        /// The site origin (e.g. `https://example.com`).
        origin: String,
    },
}

/// The `fauxx anchor ...` subcommands (C3 #19).
#[derive(Subcommand, Debug)]
pub enum AnchorCommand {
    /// Record (or update) a curated account anchor in the inventory.
    Record {
        /// A human label for the account.
        label: String,
        /// The site/service host or label.
        site: String,
        /// Identity signals on the account (repeat or comma-separate).
        #[arg(long, value_enum, value_delimiter = ',')]
        signal: Vec<SignalArg>,
        /// A stable shared-contact key linking this anchor to others.
        #[arg(long)]
        shared_contact_key: Option<String>,
    },
    /// List the account-anchor inventory.
    List {
        /// Emit the inventory as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Score the inventory by identity linkage, strongest first.
    Score {
        /// Emit the scores as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Produce prioritized partitioning recommendations.
    Recommendations {
        /// Emit the recommendations as JSON.
        #[arg(long)]
        json: bool,
    },
}

/// The identity signals accepted on the CLI.
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum SignalArg {
    /// A verified email on the account.
    VerifiedEmail,
    /// A phone number on the account.
    PhoneNumber,
    /// The user's real legal name.
    LegalName,
    /// A payment instrument.
    Payment,
    /// A recovery contact bridging accounts.
    RecoveryContact,
}

impl From<SignalArg> for fauxx_core::IdentitySignal {
    fn from(arg: SignalArg) -> Self {
        match arg {
            SignalArg::VerifiedEmail => fauxx_core::IdentitySignal::VerifiedEmail,
            SignalArg::PhoneNumber => fauxx_core::IdentitySignal::PhoneNumber,
            SignalArg::LegalName => fauxx_core::IdentitySignal::LegalName,
            SignalArg::Payment => fauxx_core::IdentitySignal::Payment,
            SignalArg::RecoveryContact => fauxx_core::IdentitySignal::RecoveryContact,
        }
    }
}

/// Arguments for `fauxx export` (C4 #23).
#[derive(Args, Debug)]
pub struct ExportArgs {
    /// The persona id to export the efficacy snapshot for.
    pub persona_id: String,
    /// The output path to write the artifact to.
    #[arg(long, value_name = "PATH")]
    pub out: PathBuf,
    /// The export format.
    #[arg(long, value_enum, default_value_t = ExportFormatArg::Json)]
    pub format: ExportFormatArg,
}

/// The export formats accepted on the CLI (C4 #23).
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum ExportFormatArg {
    /// Comma-separated rows.
    Csv,
    /// Structured JSON.
    Json,
    /// A human-readable, dated PDF summary.
    Pdf,
}

impl From<ExportFormatArg> for fauxx_core::ExportFormat {
    fn from(arg: ExportFormatArg) -> Self {
        match arg {
            ExportFormatArg::Csv => fauxx_core::ExportFormat::Csv,
            ExportFormatArg::Json => fauxx_core::ExportFormat::Json,
            ExportFormatArg::Pdf => fauxx_core::ExportFormat::Pdf,
        }
    }
}

/// The `fauxx ab ...` subcommands (C4 #21).
#[derive(Subcommand, Debug)]
pub enum AbCommand {
    /// Define (insert or replace) a shadow profile.
    Define {
        /// A human-readable label for the profile.
        label: String,
        /// The persona id this profile drives.
        persona_id: String,
        /// Which experimental arm this profile belongs to.
        #[arg(long, value_enum, default_value_t = ArmArg::Treated)]
        arm: ArmArg,
        /// Override the shadow-profile id (default: a fresh UUID v4).
        #[arg(long)]
        id: Option<String>,
    },
    /// List the defined shadow profiles.
    List {
        /// Emit the profiles as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Compare the treated and control cohorts on a platform.
    Compare {
        /// The persona id whose declared interests form the drift baseline.
        persona_id: String,
        /// The platform to compare on.
        #[arg(long, value_enum, default_value_t = PlatformArg::Google)]
        platform: PlatformArg,
        /// Emit the comparison as JSON.
        #[arg(long)]
        json: bool,
    },
}

/// The shadow-profile arms accepted on the CLI (C4 #21).
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum ArmArg {
    /// A treated (noised) profile.
    Treated,
    /// An untreated control profile.
    Control,
}

impl From<ArmArg> for fauxx_core::Arm {
    fn from(arg: ArmArg) -> Self {
        match arg {
            ArmArg::Treated => fauxx_core::Arm::Treated,
            ArmArg::Control => fauxx_core::Arm::Control,
        }
    }
}

/// The built-in platforms accepted on the CLI (C4 #20).
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum PlatformArg {
    /// Google, driven by Privacy Sandbox Topics read-backs.
    Google,
    /// Data brokers, driven by the broker submission history.
    Brokers,
    /// Meta (no desktop data source yet; an empty series).
    Meta,
}

impl From<PlatformArg> for fauxx_core::Platform {
    fn from(arg: PlatformArg) -> Self {
        match arg {
            PlatformArg::Google => fauxx_core::Platform::Google,
            PlatformArg::Brokers => fauxx_core::Platform::Brokers,
            PlatformArg::Meta => fauxx_core::Platform::Meta,
        }
    }
}

/// Arguments for `fauxx drift` (C4 #20).
#[derive(Args, Debug)]
pub struct DriftArgs {
    /// The persona id to compute drift for.
    pub persona_id: String,
    /// Emit the per-platform drift as JSON.
    #[arg(long)]
    pub json: bool,
}

/// The `fauxx pack ...` subcommands (C5 #27).
#[derive(Subcommand, Debug)]
pub enum PackCommand {
    /// Export selected personas to a signed pack file.
    Export {
        /// The output path to write the signed pack to.
        #[arg(long, value_name = "PATH")]
        out: PathBuf,
        /// The persona ids to include (repeat or comma-separate).
        #[arg(long, value_name = "ID", value_delimiter = ',', required = true)]
        persona: Vec<String>,
        /// A free-form note recorded in the pack provenance.
        #[arg(long)]
        note: Option<String>,
    },
    /// Import a signed pack file (verify, then land its personas).
    Import {
        /// The pack file to import.
        #[arg(value_name = "PATH")]
        path: PathBuf,
    },
    /// List the installed persona packs (the library ledger).
    List {
        /// Emit the installed packs as JSON.
        #[arg(long)]
        json: bool,
    },
}

/// Arguments for `fauxx generate` (C6 #28).
#[derive(Args, Debug)]
pub struct GenerateArgs {
    /// The persona id to run the generation pass for.
    pub persona_id: String,
    /// The decoy intensity to plan at.
    #[arg(long, value_enum, default_value_t = IntensityArg::Medium)]
    pub intensity: IntensityArg,
    /// Seed making the pass deterministic.
    #[arg(long, default_value_t = 0)]
    pub seed: u64,
    /// Also push the signed artifacts to every paired peer.
    #[arg(long)]
    pub push: bool,
}

/// The intensity ladder accepted on the CLI.
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum IntensityArg {
    /// 12 actions/hour.
    Low,
    /// 60 actions/hour.
    Medium,
    /// 200 actions/hour.
    High,
    /// 500 actions/hour.
    Extreme,
}

impl From<IntensityArg> for fauxx_core::IntensityLevel {
    fn from(arg: IntensityArg) -> Self {
        match arg {
            IntensityArg::Low => fauxx_core::IntensityLevel::Low,
            IntensityArg::Medium => fauxx_core::IntensityLevel::Medium,
            IntensityArg::High => fauxx_core::IntensityLevel::High,
            IntensityArg::Extreme => fauxx_core::IntensityLevel::Extreme,
        }
    }
}

/// Arguments for `fauxx mint` (C6 #29).
#[derive(Args, Debug)]
pub struct MintArgs {
    /// How many coherent personas to mint.
    pub count: usize,
    /// The output path to write the signed pack to.
    #[arg(long, value_name = "PATH")]
    pub out: PathBuf,
    /// Seed making the draw reproducible.
    #[arg(long, default_value_t = 0)]
    pub seed: u64,
    /// Also push the signed pack to every paired peer.
    #[arg(long)]
    pub push: bool,
}

/// The `fauxx egress ...` subcommands (C7 #30).
#[derive(Subcommand, Debug)]
pub enum EgressCommand {
    /// Bind a per-persona egress.
    Set {
        /// The persona id.
        persona_id: String,
        /// The egress kind.
        #[arg(value_enum)]
        kind: EgressKindArg,
        /// Proxy/VPN host (for http/socks/vpn kinds).
        #[arg(long)]
        host: Option<String>,
        /// Proxy/VPN port (for http/socks kinds).
        #[arg(long)]
        port: Option<u16>,
        /// The Tor SOCKS5 address (for the tor kind; default 127.0.0.1:9050).
        #[arg(long)]
        socks_addr: Option<String>,
    },
    /// Show a persona's bound egress and its exit indicator.
    Get {
        /// The persona id.
        persona_id: String,
        /// Emit the exit indicator as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Clear a persona's egress binding (revert to direct).
    Clear {
        /// The persona id.
        persona_id: String,
    },
}

/// The egress kinds accepted on the CLI (C7 #30).
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum EgressKindArg {
    /// The OS default route (no proxy).
    Direct,
    /// An HTTP/HTTPS proxy exit.
    Http,
    /// A SOCKS5 proxy exit.
    Socks,
    /// A local Tor SOCKS5 front.
    Tor,
}

/// The `fauxx dns ...` subcommands (C7 #31).
#[derive(Subcommand, Debug)]
pub enum DnsCommand {
    /// Bind a per-persona DNS strategy.
    Set {
        /// The persona id.
        persona_id: String,
        /// The DNS mode.
        #[arg(value_enum)]
        mode: DnsModeArg,
        /// The resolver endpoint (required for doh/dot).
        #[arg(long)]
        resolver: Option<String>,
    },
    /// Show a persona's bound DNS strategy and its observer note.
    Get {
        /// The persona id.
        persona_id: String,
        /// Emit the strategy as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Verify the secure-DNS routing a persona's decoy launch applies: the
    /// strategy, resolver, and the exact Chromium secure-DNS flags, so an
    /// operator can confirm lookups are configured to leave via the intended
    /// resolver (C7 #31).
    Verify {
        /// The persona id.
        persona_id: String,
    },
}

/// The DNS modes accepted on the CLI (C7 #31).
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum DnsModeArg {
    /// The OS default resolver.
    System,
    /// DNS-over-HTTPS to an explicit resolver.
    Doh,
    /// DNS-over-TLS to an explicit resolver.
    Dot,
}

/// The `fauxx campaign ...` subcommands (C8 #33).
#[derive(Subcommand, Debug)]
pub enum CampaignCommand {
    /// Create a goal-driven campaign.
    Create {
        /// A human-readable label.
        label: String,
        /// The persona id the campaign drives.
        persona_id: String,
        /// The target segment/category name (a CategoryPool name).
        target_segment: String,
        /// The goal comparator.
        #[arg(long, value_enum, default_value_t = ComparatorArg::AtMost)]
        comparator: ComparatorArg,
        /// The goal threshold (must be finite).
        #[arg(long)]
        threshold: f64,
        /// Override the campaign id (default: a fresh UUID v4).
        #[arg(long)]
        id: Option<String>,
    },
    /// List campaigns (optionally scoped to a persona).
    List {
        /// Scope the list to one persona id.
        #[arg(long)]
        persona: Option<String>,
        /// Emit the campaigns as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Start (or resume) a campaign.
    Start {
        /// The campaign id.
        id: String,
    },
    /// Pause a campaign on user request.
    Pause {
        /// The campaign id.
        id: String,
    },
    /// Adjust a campaign's goal threshold.
    Adjust {
        /// The campaign id.
        id: String,
        /// The new threshold (must be finite).
        threshold: f64,
    },
    /// Advance a campaign's closed loop one tick.
    Tick {
        /// The campaign id.
        id: String,
        /// Emit the directive as JSON.
        #[arg(long)]
        json: bool,
    },
}

/// The goal comparators accepted on the CLI (C8 #33).
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum ComparatorArg {
    /// Goal met when observed >= threshold (drive the metric UP).
    AtLeast,
    /// Goal met when observed <= threshold (drive the metric DOWN).
    AtMost,
}

impl From<ComparatorArg> for fauxx_core::Comparator {
    fn from(arg: ComparatorArg) -> Self {
        match arg {
            ComparatorArg::AtLeast => fauxx_core::Comparator::AtLeast,
            ComparatorArg::AtMost => fauxx_core::Comparator::AtMost,
        }
    }
}

/// Arguments for `fauxx serve` (C8 #35, the homelab mode).
#[derive(Args, Debug)]
pub struct ServeArgs {
    /// Override the serve config-file path (default: the per-OS config dir, see
    /// `docs/DEPLOYMENT.md`). The file is JSON; a missing file uses defaults.
    #[arg(long, value_name = "PATH", env = "FAUXX_SERVE_CONFIG")]
    pub config: Option<PathBuf>,

    /// Print the effective config (merged with defaults) and exit, without
    /// opening the store or running the loop.
    #[arg(long)]
    pub check: bool,

    /// Enable the Home Assistant MQTT bridge (overrides the config flag). Only
    /// effective when the binary is built with the `mqtt` feature.
    #[arg(long)]
    pub mqtt: bool,

    /// Enable live LAN persona sync (C1 #7, overrides the config flag): advertise
    /// over mDNS, browse for paired peers, and listen on the sync port for sealed
    /// inbound persona frames. Off by default (no sockets opened, nothing
    /// advertised).
    #[arg(long)]
    pub lan_sync: bool,

    /// Run a fixed number of tick iterations then exit (for tests / one-shot
    /// runs). Omit for the unbounded long-running loop.
    #[arg(long, value_name = "N")]
    pub max_ticks: Option<u64>,
}

/// The `fauxx pair ...` subcommands.
#[derive(Subcommand, Debug)]
pub enum PairCommand {
    /// Print this device's pairing QR (unicode), fingerprint, and raw payload.
    Show,

    /// Complete pairing from a scanned payload string (the QR contents).
    Add {
        /// The base64url pairing payload scanned from the peer's QR.
        payload: String,
    },
}

/// The `fauxx mode ...` subcommand. Absent (bare `fauxx mode`) also shows the
/// current mode.
#[derive(Subcommand, Debug)]
pub enum ModeCommand {
    /// Show the current coordination mode (same as bare `fauxx mode`).
    Show,
    /// Set the coordination mode.
    Set {
        /// The mode to set: `coherent` or `fragmentation`.
        mode: ModeArg,
    },
}

/// The lowercase short forms accepted for the coordination mode, mapped to
/// [`fauxx_core::CoordinationMode`] by the command handler.
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum ModeArg {
    /// One shared persona across the whole household, advancing together.
    Coherent,
    /// A distinct persona per device, with independent timing.
    Fragmentation,
}

impl From<ModeArg> for fauxx_core::CoordinationMode {
    fn from(arg: ModeArg) -> Self {
        match arg {
            ModeArg::Coherent => fauxx_core::CoordinationMode::CoherentHousehold,
            ModeArg::Fragmentation => fauxx_core::CoordinationMode::Fragmentation,
        }
    }
}

/// The `fauxx persona ...` subcommands.
#[derive(Subcommand, Debug)]
pub enum PersonaCommand {
    /// List stored personas.
    List {
        /// Emit the full persona list as JSON instead of a summary table.
        #[arg(long)]
        json: bool,
    },

    /// Show one persona by id (pretty-printed JSON).
    Show {
        /// The persona id (UUID) to display.
        id: String,
    },

    /// Add a persona (from JSON, or from individual flags) so list/show are
    /// demonstrable end to end. Full persona management lands in C5.
    Add(PersonaAddArgs),

    /// Delete a persona by id.
    Delete {
        /// The persona id (UUID) to delete.
        id: String,
    },
}

/// Arguments for `fauxx persona add`.
///
/// Either supply a complete persona via `--from-json <FILE>` (or `-` for
/// stdin), or build one from the individual `--name`/`--age-range`/etc. flags.
/// The JSON form and the flag form are mutually exclusive.
#[derive(Args, Debug, Default)]
pub struct PersonaAddArgs {
    /// Read a complete persona JSON document from this file (`-` for stdin).
    /// Mutually exclusive with the field flags below.
    #[arg(
        long,
        value_name = "FILE",
        conflicts_with_all = ["name", "age_range", "profession", "region", "interests"]
    )]
    pub from_json: Option<PathBuf>,

    /// Display name.
    #[arg(long, required_unless_present = "from_json")]
    pub name: Option<String>,

    /// Age-range enum name (e.g. `AGE_35_44`).
    #[arg(long, value_name = "AGE_RANGE", required_unless_present = "from_json")]
    pub age_range: Option<String>,

    /// Profession enum name (e.g. `ENGINEER`).
    #[arg(long, required_unless_present = "from_json")]
    pub profession: Option<String>,

    /// Region enum name (e.g. `US_WEST`).
    #[arg(long, required_unless_present = "from_json")]
    pub region: Option<String>,

    /// Interest category enum names; repeat the flag or pass a comma list.
    #[arg(
        long,
        value_name = "CATEGORY",
        value_delimiter = ',',
        required_unless_present = "from_json"
    )]
    pub interests: Vec<String>,

    /// Override the persona id (default: a fresh UUID v4).
    #[arg(long)]
    pub id: Option<String>,

    /// Optional desktop-only freeform note.
    #[arg(long)]
    pub note: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sibling_key_path_appends_dot_key() {
        let db = Path::new("/data/fauxx.db");
        assert_eq!(sibling_key_path(db), PathBuf::from("/data/fauxx.db.key"));
    }

    #[test]
    fn default_key_source_is_os_keystore() -> anyhow::Result<()> {
        let opts = StoreOpts::default();
        // No passphrase flags resolve to None (OS keystore).
        assert!(opts.resolve_key_source()?.is_none());
        Ok(())
    }

    #[test]
    fn passphrase_file_selects_encrypted_file_source() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let pass = dir.path().join("pass.txt");
        std::fs::write(&pass, "hunter2\n")?;
        let opts = StoreOpts {
            db: Some(dir.path().join("fauxx.db")),
            passphrase_file: Some(pass),
            ..Default::default()
        };
        match opts.resolve_key_source()? {
            Some(KeySource::EncryptedFile { path, passphrase }) => {
                assert_eq!(path, dir.path().join("fauxx.db.key"));
                assert_eq!(passphrase, "hunter2");
            }
            other => anyhow::bail!("expected EncryptedFile source, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn explicit_key_file_overrides_sibling() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let pass = dir.path().join("pass.txt");
        std::fs::write(&pass, "pw")?;
        let key = dir.path().join("custom.key");
        let opts = StoreOpts {
            db: Some(dir.path().join("fauxx.db")),
            passphrase_file: Some(pass),
            key_file: Some(key.clone()),
            ..Default::default()
        };
        match opts.resolve_key_source()? {
            Some(KeySource::EncryptedFile { path, .. }) => assert_eq!(path, key),
            other => anyhow::bail!("expected EncryptedFile source, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn empty_passphrase_file_is_an_error() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let pass = dir.path().join("pass.txt");
        std::fs::write(&pass, "\n")?;
        let opts = StoreOpts {
            db: Some(dir.path().join("fauxx.db")),
            passphrase_file: Some(pass),
            ..Default::default()
        };
        assert!(opts.resolve_key_source().is_err());
        Ok(())
    }

    #[test]
    fn cli_parses_and_help_is_valid() {
        use clap::CommandFactory;
        // Catches clap configuration errors (conflicting args, bad value-delim).
        Cli::command().debug_assert();
    }
}
