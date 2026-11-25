//! Command-line interface boundary.
//!
//! Coordinates user-facing CLI invocation and delegates execution to
//! application subsystems without owning playback or library internals.

/// Returns the application name and package version.
pub fn app_banner() -> String {
    format!("{} v{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
}

/// Runs the Termwave CLI entry flow.
pub fn run() -> anyhow::Result<()> {
    println!("{} — terminal Hi-Fi music player", app_banner());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_banner_includes_name_and_version() {
        let banner = app_banner();
        assert!(banner.starts_with("termwave v"));
        assert!(banner.contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn cli_run_succeeds() {
        assert!(run().is_ok());
    }
}
