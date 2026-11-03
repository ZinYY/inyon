//! `inyon`, the command: everything is in the library (`cli.rs`). On Windows, inyon-app.exe is a
//! copy of this program marked to run without a console (`engine::make_copy`).

fn main() {
    inyon_cli::cli::main()
}
