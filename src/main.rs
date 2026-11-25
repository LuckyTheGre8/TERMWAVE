//! Termwave — a lightweight, cross-platform, lossless-first terminal music player.

/// Returns the application name and package version.
fn app_banner() -> String {
    format!("{} v{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
}

fn main() {
    println!("{} — terminal Hi-Fi music player", app_banner());
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
}
