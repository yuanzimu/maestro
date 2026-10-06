use extract_config::{load_client_config, load_config, load_server_config};

const CFG: &str = "host=example.com\nport=8080\n";

#[test]
fn unified_config_parses() {
    assert_eq!(load_config(CFG), Some(("example.com".into(), 8080)));
}

#[test]
fn unified_config_rejects_incomplete() {
    assert_eq!(load_config("host=example.com\n"), None);
    assert_eq!(load_config("port=8080\n"), None);
    assert_eq!(load_config(""), None);
}

#[test]
fn unified_config_rejects_bad_port() {
    assert_eq!(load_config("host=h\nport=abc\n"), None);
}

#[test]
fn old_apis_behavior_preserved() {
    assert_eq!(load_server_config(CFG), Some(("example.com".into(), 8080)));
    assert_eq!(load_client_config(CFG), Some(("example.com".into(), 8080)));
}
