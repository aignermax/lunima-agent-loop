use super::*;

fn doc(json: &str) -> (tempfile::TempDir, ConfigDoc) {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("agent-loop.json");
    std::fs::write(&path, json).unwrap();
    let d = ConfigDoc::load(&path).unwrap();
    (tmp, d)
}

fn spec(key: &str) -> &'static FieldSpec {
    FIELDS.iter().find(|s| s.key == key).unwrap()
}

#[test]
fn unknown_fields_and_order_survive_a_save() {
    let (_t, mut d) = doc(r#"{"zeta":1,"enabled":false,"custom":{"x":[1,2]}}"#);
    d.set_from_text(spec("enabled"), "true").unwrap();
    d.save().unwrap();
    let text = std::fs::read_to_string(&d.path).unwrap();
    let keys: Vec<_> = ConfigDoc::load(&d.path).unwrap().map.keys().cloned().collect();
    assert_eq!(keys, ["zeta", "enabled", "custom"]);
    assert!(text.contains("\"enabled\": true"));
    assert!(text.contains("\"x\""));
}

#[test]
fn number_fields_reject_garbage() {
    let (_t, mut d) = doc(r#"{"maxTasksPerDay":2}"#);
    assert!(d.set_from_text(spec("maxTasksPerDay"), "zwölf").is_err());
    d.set_from_text(spec("maxTasksPerDay"), " 12 ").unwrap();
    assert_eq!(d.int("maxTasksPerDay"), Some(12));
}

#[test]
fn reads_like_the_loop_bom_comments_casing_defaults() {
    let (_t, d) = doc("\u{feff}{\n  // repo\n  \"GitHubRepo\": \"a/b\", /* x */ \"clonePath\": \"C:/c//d\"\n}");
    assert!(d.had_comments);
    assert_eq!(d.str("githubRepo"), "a/b");
    assert_eq!(d.str("clonePath"), "C:/c//d", "// inside a string is not a comment");
    assert!(d.bool("enabled"), "missing enabled defaults to true like LoopConfig");
    assert_eq!(d.str("taskLabel"), "agent-task");
    assert_eq!(d.int("maxTasksPerDay"), Some(2));
}

#[test]
fn validate_mirrors_loopconfig_load() {
    let (_t, mut d) = doc(r#"{"clonePath":""}"#);
    assert!(d.validate().is_err());
    d.map.insert("clonePath".into(), "C:/x".into());
    assert!(d.validate().is_ok());
    d.map.insert("customerEnabled".into(), true.into());
    d.map.insert("customerMaxReviewsPerCycle".into(), 0.into());
    assert!(d.validate().unwrap_err().contains("customerMaxReviewsPerCycle"));
}

#[test]
fn setting_a_field_keeps_the_files_key_casing() {
    let (_t, mut d) = doc(r#"{"Enabled":false}"#);
    d.set_from_text(spec("enabled"), "true").unwrap();
    assert_eq!(d.map.keys().collect::<Vec<_>>(), ["Enabled"]);
    assert!(d.bool("enabled"));
}

#[test]
fn bool_toggle_keeps_comments_and_layout() {
    let (_t, d) = doc("{\n  // worker switch\n  \"clonePath\": \"C:/c\",\n  \"WorkersEnabled\": true /* loop */\n}\n");
    ConfigDoc::set_bool_in_file(&d.path, "workersEnabled", false).unwrap();
    let text = std::fs::read_to_string(&d.path).unwrap();
    assert!(text.contains("// worker switch") && text.contains("/* loop */"));
    assert!(!ConfigDoc::load(&d.path).unwrap().bool("workersEnabled"));
    // missing key is inserted
    ConfigDoc::set_bool_in_file(&d.path, "customerEnabled", false).unwrap();
    assert!(!ConfigDoc::load(&d.path).unwrap().bool("customerEnabled"));
    assert!(std::fs::read_to_string(&d.path).unwrap().contains("// worker switch"));
}

#[test]
fn invalid_json_reports_error() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("agent-loop.json");
    std::fs::write(&path, "{ nope").unwrap();
    assert!(ConfigDoc::load(&path).is_err());
}
