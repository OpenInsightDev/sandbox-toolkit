use clap::Parser;
use sbx::Cli;

#[test]
fn test_cli_default_values() {
    let cli = Cli::try_parse_from(["sbx", "/tmp/test"]).unwrap();
    assert_eq!(cli.root_dir, "/tmp/test");
    assert_eq!(cli.host, "0.0.0.0");
    assert_eq!(cli.port, None);
    assert_eq!(cli.effective_port(), 8080);
}

#[test]
fn test_cli_custom_host_short() {
    let cli = Cli::try_parse_from(["sbx", "-H", "127.0.0.1", "/tmp/test"]).unwrap();
    assert_eq!(cli.host, "127.0.0.1");
}

#[test]
fn test_cli_custom_host_long() {
    let cli = Cli::try_parse_from(["sbx", "--host", "127.0.0.1", "/tmp/test"]).unwrap();
    assert_eq!(cli.host, "127.0.0.1");
}

#[test]
fn test_cli_custom_port_short() {
    let cli = Cli::try_parse_from(["sbx", "-p", "3000", "/tmp/test"]).unwrap();
    assert_eq!(cli.port, Some(3000));
}

#[test]
fn test_cli_custom_port_long() {
    let cli = Cli::try_parse_from(["sbx", "--port", "3000", "/tmp/test"]).unwrap();
    assert_eq!(cli.port, Some(3000));
}

#[test]
fn test_cli_full() {
    let cli = Cli::try_parse_from([
        "sbx",
        "--host",
        "127.0.0.1",
        "--port",
        "9090",
        "/srv/webdav",
    ])
    .unwrap();
    assert_eq!(cli.root_dir, "/srv/webdav");
    assert_eq!(cli.host, "127.0.0.1");
    assert_eq!(cli.port, Some(9090));
}

#[test]
fn test_cli_root_dir_default() {
    let cli = Cli::try_parse_from(["sbx"]).unwrap();
    assert_eq!(cli.root_dir, ".");
}

#[test]
fn test_cli_root_dir_custom() {
    let cli = Cli::try_parse_from(["sbx", "/custom/path"]).unwrap();
    assert_eq!(cli.root_dir, "/custom/path");
}

#[test]
fn test_cli_user_single() {
    let cli = Cli::try_parse_from(["sbx", "--user", "admin:secret", "/tmp/test"]).unwrap();
    assert_eq!(cli.users.len(), 1);
    assert_eq!(cli.users[0], "admin:secret");
}

#[test]
fn test_cli_user_multiple() {
    let cli = Cli::try_parse_from([
        "sbx",
        "--user",
        "admin:secret",
        "--user",
        "viewer:public",
        "/tmp/test",
    ])
    .unwrap();
    assert_eq!(cli.users.len(), 2);
    assert_eq!(cli.users[0], "admin:secret");
    assert_eq!(cli.users[1], "viewer:public");
}

#[test]
fn test_cli_to_auth_state() {
    let cli = Cli::try_parse_from([
        "sbx",
        "--user",
        "alice:pass1",
        "--user",
        "bob:pass2",
        "/tmp/test",
    ])
    .unwrap();
    let auth = cli.to_auth_state();
    assert!(!auth.is_empty());
    assert!(auth.validate("alice", "pass1"));
    assert!(auth.validate("bob", "pass2"));
    assert!(!auth.validate("alice", "wrong"));
    assert!(!auth.validate("eve", "pass1"));
}

#[test]
fn test_cli_to_auth_state_empty() {
    let cli = Cli::try_parse_from(["sbx", "/tmp/test"]).unwrap();
    let auth = cli.to_auth_state();
    assert!(auth.is_empty());
}

#[test]
fn test_cli_to_auth_state_skips_malformed() {
    let cli =
        Cli::try_parse_from(["sbx", "--user", "bob", "--user", "alice:pass", "/tmp/test"]).unwrap();
    let auth = cli.to_auth_state();
    assert!(auth.validate("alice", "pass"));
    assert!(!auth.is_empty());
}

#[test]
fn test_cli_to_auth_state_skips_empty_username() {
    let cli = Cli::try_parse_from(["sbx", "--user", ":password", "/tmp/test"]).unwrap();
    let auth = cli.to_auth_state();
    assert!(auth.is_empty());
}

#[test]
fn test_cli_combined_flags() {
    let cli = Cli::try_parse_from([
        "sbx",
        "--user",
        "u:p",
        "-H",
        "127.0.0.1",
        "-p",
        "9999",
        "/data",
    ])
    .unwrap();
    assert_eq!(cli.host, "127.0.0.1");
    assert_eq!(cli.port, Some(9999));
    assert_eq!(cli.root_dir, "/data");
    assert_eq!(cli.users.len(), 1);
}

#[test]
fn test_cli_log_level_default() {
    let cli = Cli::try_parse_from(["sbx", "/tmp/test"]).unwrap();
    assert!(!cli.quiet);
    assert_eq!(cli.verbose, 0);

    #[cfg(debug_assertions)]
    assert_eq!(cli.log_level(), "debug");
    #[cfg(not(debug_assertions))]
    assert_eq!(cli.log_level(), "info");
}

#[test]
fn test_cli_log_level_verbose_short() {
    let cli = Cli::try_parse_from(["sbx", "-v", "/tmp/test"]).unwrap();
    assert_eq!(cli.verbose, 1);
    assert_eq!(cli.log_level(), "debug");
}

#[test]
fn test_cli_log_level_verbose_long() {
    let cli = Cli::try_parse_from(["sbx", "--verbose", "/tmp/test"]).unwrap();
    assert_eq!(cli.verbose, 1);
    assert_eq!(cli.log_level(), "debug");
}

#[test]
fn test_cli_log_level_very_verbose_short() {
    let cli = Cli::try_parse_from(["sbx", "-vv", "/tmp/test"]).unwrap();
    assert_eq!(cli.verbose, 2);
    assert_eq!(cli.log_level(), "trace");
}

#[test]
fn test_cli_log_level_very_verbose_short_repeated() {
    let cli = Cli::try_parse_from(["sbx", "-v", "-v", "/tmp/test"]).unwrap();
    assert_eq!(cli.verbose, 2);
    assert_eq!(cli.log_level(), "trace");
}

#[test]
fn test_cli_log_level_very_verbose_excess() {
    let cli = Cli::try_parse_from(["sbx", "-vvvv", "/tmp/test"]).unwrap();
    assert_eq!(cli.verbose, 4);
    assert_eq!(cli.log_level(), "trace");
}

#[test]
fn test_cli_log_level_quiet_short() {
    let cli = Cli::try_parse_from(["sbx", "-q", "/tmp/test"]).unwrap();
    assert!(cli.quiet);
    assert_eq!(cli.log_level(), "off");
}

#[test]
fn test_cli_log_level_quiet_long() {
    let cli = Cli::try_parse_from(["sbx", "--quiet", "/tmp/test"]).unwrap();
    assert!(cli.quiet);
    assert_eq!(cli.log_level(), "off");
}

#[test]
fn test_cli_log_level_verbose_conflicts_with_quiet() {
    assert!(Cli::try_parse_from(["sbx", "-v", "-q", "/tmp/test"]).is_err());
}

#[test]
fn test_cli_log_level_verbose_long_conflicts_with_quiet() {
    assert!(Cli::try_parse_from(["sbx", "--verbose", "--quiet", "/tmp/test"]).is_err());
}

#[test]
fn test_shadow_file_parse_default_rw() {
    let cli = Cli::try_parse_from(["sbx", "--shadow-file", "/etc/sbx/shadow"]).unwrap();
    let arg = cli.to_shadow_file_arg().unwrap();
    assert_eq!(arg.path, "/etc/sbx/shadow");
    assert!(arg.writable);
}

#[test]
fn test_shadow_file_parse_explicit_ro() {
    let cli = Cli::try_parse_from(["sbx", "--shadow-file", "/etc/sbx/shadow:ro"]).unwrap();
    let arg = cli.to_shadow_file_arg().unwrap();
    assert_eq!(arg.path, "/etc/sbx/shadow");
    assert!(!arg.writable);
}

#[test]
fn test_shadow_file_parse_rw() {
    let cli = Cli::try_parse_from(["sbx", "--shadow-file", "/etc/sbx/shadow:rw"]).unwrap();
    let arg = cli.to_shadow_file_arg().unwrap();
    assert_eq!(arg.path, "/etc/sbx/shadow");
    assert!(arg.writable);
}

#[test]
fn test_shadow_file_none() {
    let cli = Cli::try_parse_from(["sbx", "/tmp/test"]).unwrap();
    assert!(cli.to_shadow_file_arg().is_none());
    assert!(!cli.shadow_write);
}

#[test]
fn test_shadow_write_flag() {
    let cli =
        Cli::try_parse_from(["sbx", "--shadow-file", "/etc/sbx/shadow", "--shadow-write"]).unwrap();
    assert!(cli.shadow_write);
}

#[test]
fn test_shadow_write_short_flags() {
    let cli = Cli::try_parse_from(["sbx", "-S", "/etc/sbx/shadow:rw", "-W", "/tmp/test"]).unwrap();
    let arg = cli.to_shadow_file_arg().unwrap();
    assert_eq!(arg.path, "/etc/sbx/shadow");
    assert!(arg.writable);
    assert!(cli.shadow_write);
}

#[test]
fn test_shadow_write_requires_shadow_file() {
    let result = Cli::try_parse_from(["sbx", "--shadow-write", "/tmp/test"]);
    assert!(result.is_err());
}
