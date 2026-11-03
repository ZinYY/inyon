use std::io::IsTerminal;

use clap::{Args, Parser, Subcommand};

use crate::commands::{self, SetupFlags};
use crate::service::{Role, Sys};
use crate::sys::RealProc;
use crate::ui::{dashboard, Theme};
use crate::{engine, pages, paths, temp, update};

/// Drive your coding agents from the Inyon app: pair this machine, keep it reachable, keep it up to date.
#[derive(Parser)]
#[command(name = "inyon", version, disable_version_flag = true, about, long_about = None, after_help = "Docs: https://inyon.tech/docs/ (or here: inyon docs <topic>)")]
struct Cli {
    /// Answer as JSON
    #[arg(long, global = true)]
    json: bool,
    /// Print the version
    #[arg(short = 'V', long)]
    version: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum CompanionAction {
    /// Start it in a tmux session called companion on the hub's machine
    Start {
        /// claude or codex
        #[arg(long, default_value = "claude")]
        agent: String,
    },
    /// End its session and turn it off
    Stop,
}

#[derive(Subcommand)]
enum Command {
    /// What this machine is to the app, where it answers, who's paired
    Status {
        /// Plain lines, even in a terminal
        #[arg(long)]
        plain: bool,
    },
    /// Set this machine up: a few questions, then the pairing code
    Setup(SetupArgs),
    /// Show a code to pair the phone app with this machine
    #[command(trailing_var_arg = true, allow_hyphen_values = true)]
    Pair { args: Vec<String> },
    /// List paired phones
    Phones,
    /// Unpair a phone
    Forget { phone: String },
    /// Add this machine to an Inyon hub, with the token in INYON_TOKEN
    Join {
        hub: String,
        /// Save the hub without starting the connection
        #[arg(long)]
        no_autostart: bool,
    },
    /// Leave the hub this machine was added to
    Disconnect,
    /// The browsers this machine sends notifications to
    #[command(trailing_var_arg = true, allow_hyphen_values = true)]
    Push { args: Vec<String> },
    /// The hub: start, stop or restart it in the background
    Hub {
        #[command(subcommand)]
        action: HubAction,
    },
    /// Run in the background at login, or stop
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
    /// The log of what runs in the background
    Logs {
        /// link, node or hub
        #[arg(long)]
        role: Option<String>,
        /// How many lines
        #[arg(short = 'n', long, default_value_t = 50)]
        lines: usize,
        /// Keep showing new lines
        #[arg(short = 'f', long)]
        follow: bool,
    },
    /// Check what the app needs on this machine
    Doctor,
    /// Stop everything inyon runs in the background here, until the next sign-in or `inyon setup`
    Stop,
    /// Get the newest version and restart what runs in the background
    Update {
        /// Run on its own and return at once (how the app asks for it)
        #[arg(long)]
        background: bool,
    },
    /// Take inyon off this machine (a role name keeps the old `inyon uninstall link` form)
    Uninstall {
        role: Option<String>,
        /// Remove the pairings, keys and logs too
        #[arg(long)]
        purge: bool,
        /// Don't ask anything (tmux or psmux that setup installed stays)
        #[arg(short = 'y', long)]
        yes: bool,
        /// Wait for Enter at the end (how Windows runs it from Settings, in a window of its own)
        #[arg(long, hide = true)]
        pause: bool,
    },
    /// The sites your agents make, shown in the app under Pages: add, list or remove them
    Pages {
        /// List the pages on all the hub's machines instead of this one's
        #[arg(long)]
        hub: bool,
        #[command(subcommand)]
        action: Option<PagesAction>,
    },
    /// The tmux sessions on the hub's machines, and the agent in each
    Sessions,
    /// Give an agent on any of the hub's machines a message; it goes in once the agent is free
    Send {
        /// machine/session, or machine/session:window.pane when the session runs more than one agent
        target: String,
        /// The message (- reads it from stdin)
        #[arg(required = true, allow_hyphen_values = true)]
        text: Vec<String>,
    },
    /// The latest messages of a session's conversation (on the hub's machine)
    Read {
        /// machine/session, or machine/session:window.pane when the session runs more than one agent
        target: String,
        /// How many messages
        #[arg(short = 'n', long = "lines", default_value_t = 20)]
        lines: usize,
    },
    /// Answer the question an agent is asking, by the choice's number or its words (on the hub's machine)
    Answer {
        /// machine/session, or machine/session:window.pane when the session runs more than one agent
        target: String,
        /// The choice: its number, or words from its label
        #[arg(required = true)]
        choice: Vec<String>,
    },
    /// The companion agent you talk to from the app: whether it runs, start it, stop it
    Companion {
        #[command(subcommand)]
        action: Option<CompanionAction>,
    },
    /// Temporary files your coding agents leave behind: list them, look inside, delete them
    Temp {
        #[command(subcommand)]
        action: Option<TempAction>,
    },
    /// Agent skills that come with each release: list, add or remove them
    Skill {
        #[command(subcommand)]
        action: SkillAction,
    },
    /// Read the docs here, without a browser: list them, or print one page
    Docs {
        /// A page's name or a few words from its title (hub, pages, add machines)
        topic: Vec<String>,
    },
    /// Print the version
    Version,
    /// Move services set up as tether-node to inyon's own, and off the Bun engine
    #[command(hide = true)]
    Migrate {
        /// Run on its own and return at once (how the tray icon asks for it)
        #[arg(long)]
        background: bool,
    },
    /// The old `inyon install [role]` form
    #[command(hide = true)]
    Install { role: Option<String> },
    /// The icon in the Windows notification area while `role` runs (the engine starts it)
    #[command(hide = true)]
    Tray {
        role: String,
        /// The engine it belongs to: the icon goes when it stops
        #[arg(long)]
        pid: Option<u32>,
    },
    /// The Inyon window on Windows: the status and the pairing code, opened from the tray icon
    #[command(hide = true)]
    Window {
        role: String,
        /// Show a pairing code straight away
        #[arg(long)]
        pair: bool,
    },
    /// The Inyon app on Windows: runs `roles` in the background with the icon in the notification
    /// area, and opens the Inyon window (what inyon-app.exe does when it's opened)
    #[command(hide = true)]
    App {
        /// link, node: the jobs to run
        roles: Vec<String>,
        /// Started at sign-in: no window
        #[arg(long)]
        background: bool,
        /// Open the window with a pairing code
        #[arg(long)]
        pair: bool,
        /// role:pid, a job an older version started, which the app takes over
        #[arg(long)]
        adopt: Option<String>,
    },
}

#[derive(Args)]
struct SetupArgs {
    /// Skip the questions and take the flags (and the defaults)
    #[arg(short = 'y', long)]
    yes: bool,
    /// hub, node (paired with the phone) or join
    #[arg(long)]
    role: Option<String>,
    /// For a node: tunnel, lan, vpn or a wss:// address, comma-separated. For a hub: localhost, lan, vpn or an address
    #[arg(long)]
    network: Option<String>,
    #[arg(long)]
    port: Option<u16>,
    /// Start at login and come back after a crash (the default; --no-autostart turns it off)
    #[arg(long, overrides_with = "no_autostart")]
    autostart: bool,
    #[arg(long)]
    no_autostart: bool,
    /// The hub to join, with the token in INYON_TOKEN
    #[arg(long)]
    hub: Option<String>,
    /// Don't show a pairing code at the end (the Inyon window shows its own)
    #[arg(long, hide = true)]
    no_code: bool,
}

#[derive(Subcommand)]
enum HubAction {
    /// Run the hub in the background from now on, and say how to sign in
    Start,
    /// Stop running it in the background (its records stay)
    Stop,
    Restart,
    /// Show the setup code a new hub asks for
    Code,
    /// Make a one-time code for a companion app (Legato) to pair with this hub
    CompanionCode,
    /// The hub's machines and what runs on them (here or on a joined machine)
    #[command(visible_alias = "ls")]
    Machines,
    /// Choose a new password for a forgotten one
    ResetPassword {
        password: Option<String>,
    },
    /// The same as start
    #[command(hide = true)]
    Install,
    /// The same as stop
    #[command(hide = true)]
    Uninstall,
}

#[derive(Subcommand)]
enum SkillAction {
    /// The skills in the newest release, and where each is installed
    List,
    /// Download a skill, check it, and install it for each coding agent on this machine
    Add {
        name: String,
        /// Install into this skills folder instead
        #[arg(long)]
        to: Option<std::path::PathBuf>,
        /// Replace a skill of the same name that Inyon didn't install
        #[arg(long)]
        force: bool,
    },
    /// Remove a skill that `inyon skill add` installed
    Remove {
        name: String,
        /// Remove it from this skills folder instead
        #[arg(long)]
        to: Option<std::path::PathBuf>,
    },
}

#[derive(Subcommand)]
enum PagesAction {
    /// Show a site's folder in the app under Pages (it needs an index.html)
    Add {
        /// The site's folder
        dir: std::path::PathBuf,
        /// The name the app shows (default: the site's own, its <title>, or the folder's name)
        #[arg(long)]
        name: Option<String>,
        /// The page's name in links: lowercase letters, digits and dashes (default: the site's own, or the folder's name)
        #[arg(long)]
        slug: Option<String>,
    },
    /// The pages on this machine (the same as plain `inyon pages`)
    #[command(visible_alias = "list")]
    Ls,
    /// Take a page out of the app; its folder stays as it is
    #[command(visible_alias = "remove")]
    Rm { slug: String },
    /// One file of a page, for the hub this machine is joined to (with --json)
    #[command(hide = true)]
    Get {
        slug: String,
        /// The path inside the site, one argument per folder or file name
        segments: Vec<String>,
        /// The path ended with a slash
        #[arg(long)]
        slash: bool,
        /// The biggest file to send, in bytes
        #[arg(long, default_value_t = pages::GET_MAX)]
        max: u64,
        /// The browser's If-None-Match: the file isn't sent when it still matches
        #[arg(long)]
        etag: Option<String>,
        /// Send the file as base64 lines after the answer, for a hub reading a stream (up to 64 MB)
        #[arg(long)]
        lines: bool,
    },
    /// Add the Blueprint sites under these folders that aren't listed yet, for the hub (with --json)
    #[command(hide = true)]
    Find {
        /// The folders agents work in
        dirs: Vec<std::path::PathBuf>,
        /// The agent working there
        #[arg(long, default_value = "found")]
        by: String,
    },
    /// The pages, again whenever they change, for the hub this machine is joined to (with --json)
    #[command(hide = true)]
    Watch {
        /// How often to look after a change, in ms (up to five times slower while nothing changes)
        #[arg(long, default_value_t = pages::WATCH_EVERY_MS)]
        every: u64,
        /// How long to go without printing before an empty line, in ms
        #[arg(long, default_value_t = pages::WATCH_ALIVE_MS)]
        alive: u64,
    },
}

#[derive(Subcommand)]
enum TempAction {
    /// What's there per agent, and the biggest items (the same as plain `inyon temp`)
    #[command(visible_alias = "list")]
    Ls,
    /// Every file inside one item, newest first
    Files {
        /// The item's path (or its id from `inyon temp ls --json`)
        item: String,
    },
    /// Print a text file, or say what kind of file it is
    Show {
        path: String,
        /// The biggest image to send whole with --json, in bytes
        #[arg(long, default_value_t = temp::IMAGE_MAX)]
        max: u64,
    },
    /// Delete items or files for good
    #[command(visible_alias = "remove")]
    Rm {
        #[arg(required_unless_present = "stdin")]
        paths: Vec<String>,
        /// Read the paths as a JSON list on stdin (how the hub sends many at once)
        #[arg(long, hide = true)]
        stdin: bool,
    },
    /// Delete every item older than an age; items changed in the last 30 minutes stay
    Clean {
        /// 7d, 24h, 30m or 2w
        #[arg(long, value_name = "AGE")]
        older_than: String,
        /// Don't ask
        #[arg(short = 'y', long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum ServiceAction {
    /// link, node or hub (the one this machine has, if none is given)
    Install {
        role: Option<String>,
    },
    Uninstall {
        role: Option<String>,
    },
    /// Which jobs are set up and running
    Status,
}

/// What goes straight to the engine built into this program, before the arguments are read here.
const FORWARDED: [&str; 2] = ["link", "connect"];
const HUB_OWN: [&str; 10] = ["start", "stop", "restart", "install", "uninstall", "code", "companion-code", "reset-password", "machines", "ls"];

/// `inyon hub <arg>` this program answers itself; anything else runs the hub. Help too: asking
/// for it once fetched the engine to run a hub (and failed on Windows, where none runs).
fn hub_own(arg: &str) -> bool {
    HUB_OWN.contains(&arg) || ["--json", "--help", "-h", "help"].contains(&arg)
}

/// A reader that stops early (`inyon status --plain | grep -q hub`) closes the pipe, and the next
/// print fails. Rust ignores SIGPIPE, so that failure panicked (exit 101). A command that only
/// prints gets SIGPIPE back on Unix and ends quietly, as `ls` does. The others keep it ignored: one
/// that runs the link (`inyon pair`) or writes to a program it started must see the error instead
/// of being killed. On Windows, and for those others, a failed print ends the program quietly
/// rather than as a crash.
fn closed_pipe_is_not_a_crash(only_prints: bool) {
    #[cfg(unix)]
    if only_prints {
        // SAFETY: called once at start, before any other thread exists
        unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    }
    #[cfg(not(unix))]
    let _ = only_prints;
    let crash = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let said = info.payload().downcast_ref::<String>().map(String::as_str).or_else(|| info.payload().downcast_ref::<&str>().copied()).unwrap_or("");
        // std's own words when println! can't write
        if said.starts_with("failed printing to stdout") || said.starts_with("failed printing to stderr") {
            std::process::exit(if cfg!(unix) { 141 } else { 0 });
        }
        crash(info)
    }));
}

pub fn main() {
    let mut argv: Vec<String> = std::env::args().collect();
    // started as inyon-engine (Windows' logon tasks run a windowless copy of this program under
    // that name, and older inyon commands and hubs ask for it): the engine's commands, as it answered them
    if argv.first().is_some_and(|a| inyon_engine::paths::started_as_engine(a)) {
        std::process::exit(engine::run_builtin(&argv[1..]));
    }
    // inyon-app.exe opened as it is (the Start menu, Settings → Apps' icon): the app
    let as_app = argv.first().is_some_and(|a| crate::app::started_as_app(a));
    if as_app && argv.len() == 1 {
        argv.push("app".into());
    }
    // copies an earlier update renamed aside (a running .exe can't be deleted until it has
    // stopped): this program's and the engine's, which no longer looks after its own
    if let Ok(me) = std::env::current_exe() {
        let now = crate::state::now_ms() as u64;
        update::cleanup_old(&update::RealFiles, &me, now);
        update::cleanup_old(&update::RealFiles, &update::engine_beside(&me), now);
    }
    if let Some(first) = argv.get(1) {
        // the built-in engine's own command line, which this program runs itself for pair, forget and push
        if first == "engine" {
            std::process::exit(engine::run_builtin(&argv[2..]));
        }
        if FORWARDED.contains(&first.as_str()) || first.starts_with("http://") || first.starts_with("https://") {
            // the Inyon app runs it already: a second copy would only fight it for the port
            if let Some(why) = crate::app::already_runs(&RealProc, first) {
                eprintln!("{why}");
                std::process::exit(1);
            }
            if let Some(stale) = engine::stale_engine() {
                // the Bun engine an update to 0.19 left here: nothing runs it any more
                let _ = std::fs::remove_file(stale);
            }
            engine::forward(&argv[1..]);
        }
        if first == "hub" && !argv.get(2).is_some_and(|a| hub_own(a)) {
            commands::hub_other(&argv[2..]);
        }
    }
    let cli = Cli::parse_from(&argv);
    closed_pipe_is_not_a_crash(
        cli.version
            || matches!(
                cli.command,
                None | Some(Command::Status { .. } | Command::Phones | Command::Logs { .. } | Command::Doctor | Command::Version | Command::Docs { .. } | Command::Sessions)
            ),
    );
    let proc = RealProc;
    let sys = Sys::new(&proc);
    let tty = std::io::stdout().is_terminal() && std::io::stdin().is_terminal();
    // only the screens use colour, and working it out may ask the terminal, so it waits for them
    let theme = || Theme::detect(tty);
    let json = cli.json;
    if cli.version {
        std::process::exit(commands::finish(commands::version(json), json));
    }
    let code = match cli.command {
        None | Some(Command::Status { plain: false }) if tty && !json => {
            if let Err(e) = dashboard::run_screen(&sys, theme()) {
                eprintln!("{e}");
                1
            } else {
                0
            }
        }
        None | Some(Command::Status { .. }) => {
            let s = dashboard::gather(&sys, true);
            if json {
                println!("{}", dashboard::json(&s));
            } else {
                println!("{}", dashboard::plain(&s));
            }
            0
        }
        Some(Command::Setup(a)) => {
            let flags = SetupFlags {
                yes: a.yes,
                role: a.role,
                network: a.network,
                port: a.port,
                autostart: if a.no_autostart {
                    Some(false)
                } else if a.autostart {
                    Some(true)
                } else {
                    None
                },
                hub: a.hub,
                no_code: a.no_code,
            };
            commands::finish(commands::setup(&sys, theme(), Some(&flags), tty), json)
        }
        Some(Command::Pair { args }) => commands::finish(commands::pair(&args, json), json),
        Some(Command::Phones) => commands::finish(commands::phones(), json),
        Some(Command::Forget { phone }) => commands::finish(commands::forget(&phone), json),
        Some(Command::Join { hub, no_autostart }) => {
            commands::finish(commands::join(&sys, &hub, &paths::env_var("INYON_TOKEN").unwrap_or_default(), paths::env_var("INYON_HUB_CERT").as_deref(), !no_autostart), json)
        }
        Some(Command::Disconnect) => commands::finish(commands::disconnect(&sys), json),
        Some(Command::Push { args }) => commands::finish(commands::push(&args), json),
        Some(Command::Hub { action }) => commands::finish(
            match action {
                HubAction::Start | HubAction::Install => commands::hub_start(&sys),
                HubAction::Stop | HubAction::Uninstall => commands::hub_stop(&sys),
                HubAction::Restart => commands::hub_restart(&sys),
                HubAction::Code => commands::hub_code(),
                HubAction::CompanionCode => commands::hub_companion_code(),
                HubAction::Machines => crate::hub_api::machines_command(),
                HubAction::ResetPassword { password } => commands::hub_reset_password(&sys, password.as_deref()),
            },
            json,
        ),
        Some(Command::Service { action }) => commands::finish(
            match action {
                ServiceAction::Install { role } => commands::service_install_command(&sys, role.as_deref()),
                ServiceAction::Uninstall { role } => commands::service_uninstall(&sys, role.as_deref()),
                ServiceAction::Status => commands::service_status(&sys),
            },
            json,
        ),
        Some(Command::Install { role }) => commands::finish(commands::service_install_command(&sys, role.as_deref()), json),
        Some(Command::Logs { role, lines, follow }) => commands::finish(commands::logs(&sys, role.as_deref(), lines, follow), json),
        Some(Command::Doctor) => commands::finish(commands::doctor(&sys), json),
        Some(Command::Stop) => commands::finish(commands::stop(&sys), json),
        Some(Command::Update { background }) => commands::finish(commands::update_command(&sys, background, json), json),
        Some(Command::Uninstall { role: Some(role), .. }) => commands::finish(commands::service_uninstall(&sys, Some(&role)), json),
        Some(Command::Uninstall { role: None, purge, yes, pause }) if as_app => {
            // from Settings → Apps, with no console: asked and answered in message boxes
            let _ = pause;
            crate::app::uninstall_from_settings(&sys, purge, yes)
        }
        Some(Command::Uninstall { role: None, purge, yes, pause }) => {
            let code = commands::finish(commands::uninstall_all(&sys, purge, !yes && tty), json);
            if pause {
                crate::ui::wait_for_enter();
            }
            code
        }
        Some(Command::Pages { hub: true, action: None | Some(PagesAction::Ls) }) => commands::finish(crate::hub_api::pages_command(), json),
        Some(Command::Pages { hub: true, .. }) => commands::finish(Err("--hub only lists pages: inyon pages --hub".into()), json),
        Some(Command::Sessions) => commands::finish(crate::hub_api::sessions_command(), json),
        Some(Command::Read { target, lines }) => commands::finish(crate::hub_api::read_command(&target, lines), json),
        Some(Command::Answer { target, choice }) => commands::finish(crate::hub_api::answer_command(&target, &choice.join(" ")), json),
        Some(Command::Companion { action }) => commands::finish(
            match action {
                None => crate::hub_api::companion_command(),
                Some(CompanionAction::Start { agent }) => crate::hub_api::companion_start(&agent),
                Some(CompanionAction::Stop) => crate::hub_api::companion_stop(),
            },
            json,
        ),
        Some(Command::Send { target, text }) => commands::finish(crate::hub_api::send_text(&text).and_then(|t| crate::hub_api::send_command(&target, &t)), json),
        Some(Command::Pages { hub: false, action }) => commands::finish(
            match action {
                Some(PagesAction::Add { dir, name, slug }) => pages::add_command(&dir, name.as_deref(), slug.as_deref()),
                Some(PagesAction::Rm { slug }) => pages::remove_command(&slug),
                Some(PagesAction::Ls) | None => pages::list_command(),
                Some(PagesAction::Get { slug, segments, slash, max, etag, lines: false }) => pages::get_command(&slug, &segments, slash, max.min(pages::GET_MAX), etag.as_deref()),
                Some(PagesAction::Get { slug, segments, slash, max, etag, lines: true }) => {
                    pages::get_lines_command(&slug, &segments, slash, max.min(pages::STREAM_MAX), etag.as_deref())
                }
                Some(PagesAction::Watch { every, alive }) => pages::watch_command(every, alive),
                Some(PagesAction::Find { dirs, by }) => pages::find_command(&dirs, &by),
            },
            json,
        ),
        Some(Command::Temp { action }) => match action {
            Some(TempAction::Ls) | None => commands::finish(temp::list_command(), json),
            Some(TempAction::Files { item }) => commands::finish(temp::files_command(&item), json),
            Some(TempAction::Show { path, max }) => commands::finish(temp::show_command(&path, max), json),
            Some(TempAction::Rm { paths, stdin }) => match temp::paths_from(paths, stdin) {
                Ok(paths) => {
                    let (out, partial) = temp::rm_command(&paths);
                    // something stayed: the answer says what and why, and the exit code says so too
                    commands::finish(out, json).max(i32::from(partial))
                }
                Err(e) => commands::finish(Err(e), json),
            },
            Some(TempAction::Clean { older_than, yes }) => commands::finish(temp::clean_command(&older_than, yes, tty && !json), json),
        },
        Some(Command::Skill { action }) => commands::finish(
            match action {
                SkillAction::List => commands::skill_list(),
                SkillAction::Add { name, to, force } => commands::skill_add(&name, to, force, json),
                SkillAction::Remove { name, to } => commands::skill_remove(&name, to),
            },
            json,
        ),
        Some(Command::Version) => commands::finish(commands::version(json), json),
        Some(Command::Docs { topic }) => commands::finish(crate::docs::docs_command(Some(&topic.join(" "))), json),
        Some(Command::Migrate { background }) => commands::finish(commands::migrate(&sys, background), json),
        Some(Command::Tray { role, pid }) => match Role::parse(&role) {
            Some(r) => {
                // a logon task that still starts the Bun engine (from 0.18) moves to the built-in one
                if commands::wants_hand_over(&sys) {
                    let _ = commands::migrate(&sys, true);
                }
                // an engine started the way 0.20.2 and earlier did it, by its own logon task: from
                // 0.20.3 the Inyon app runs the jobs, so it takes this one over
                match crate::app::take_over(&sys, r, pid) {
                    Some(code) => code,
                    None => crate::tray::run(r, pid),
                }
            }
            None => {
                eprintln!("{role} isn't link, node or hub");
                2
            }
        },
        Some(Command::Window { role, pair }) => match Role::parse(&role) {
            Some(r) => {
                crate::desktop::trace("opening");
                crate::app::open_window(r, pair)
            }
            None => {
                eprintln!("{role} isn't link, node or hub");
                2
            }
        },
        Some(Command::App { roles, background, pair, adopt }) => {
            let mut list = Vec::new();
            for r in &roles {
                match Role::parse(r).filter(|r| *r != Role::Hub) {
                    Some(r) => list.push(r),
                    None => {
                        eprintln!("{r} isn't link or node");
                        std::process::exit(2);
                    }
                }
            }
            let adopt = adopt.as_deref().and_then(|a| a.split_once(':')).and_then(|(r, p)| Some((Role::parse(r)?, p.parse::<u32>().ok()?)));
            crate::app::run(&sys, list, background, pair, adopt)
        }
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    /// A `hub` subcommand left out of HUB_OWN would go to the engine, which runs a hub instead.
    #[test]
    fn every_hub_subcommand_is_answered_here() {
        let cmd = super::Cli::command();
        let hub = cmd.find_subcommand("hub").unwrap();
        for sub in hub.get_subcommands() {
            for name in std::iter::once(sub.get_name()).chain(sub.get_all_aliases()) {
                assert!(super::hub_own(name), "inyon hub {name}");
            }
        }
    }
}
