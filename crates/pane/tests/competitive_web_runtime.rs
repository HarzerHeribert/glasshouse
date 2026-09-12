//! The shipped V8 capability surface must expose the broker and fail closed
//! before configuration, including in helper runtimes.
use pane::contract::SessionId;
use pane::glasshouse::Glasshouse;
use pane::runtime::isolate::Runtime;
use pane::runtime::outcome::CellOutcome;
use pane::runtime::preview::Value;
use pane::sandbox::profile::Profile;

#[test]
fn web_callbacks_are_reachable_and_disabled_without_host_configuration() {
    let root = std::env::current_dir().unwrap();
    let mut runtime = Runtime::new(
        &Profile::compile(&root, None),
        &Glasshouse::None,
        &SessionId::new("web-disabled"),
    );
    let outcome = runtime.run_cell(
        "try { web.fetch('https://example.com'); } catch (error) { return error.message; }",
    );
    match outcome {
        CellOutcome::Returned {
            value: Value::String(value),
            ..
        } => {
            assert!(format!("{value:?}").contains("web access is disabled"));
        }
        other => panic!("expected disabled broker error, got {other:?}"),
    }
}

#[test]
fn web_is_withheld_from_helpers_and_declared_to_parent() {
    assert!(!pane::runtime::bindings::HostGlobals::Helper(&[]).installs("web"));
    assert!(
        pane::prompt::declarations::RUNTIME
            .iter()
            .any(|entry| entry.global == "web" && entry.declaration.contains("search(query"))
    );
}

#[test]
fn web_configuration_is_explicit_and_rejects_unknown_fields() {
    let config =
        pane::config::PaneConfig::parse("[web]\nenabled = true\nallow_domains = ['example.com']\n")
            .unwrap();
    assert!(config.web.enabled);
    assert!(!pane::config::PaneConfig::default().web.enabled);
    assert!(pane::config::PaneConfig::parse("[web]\nallow_everything = true\n").is_err());
}

struct FixtureTransport;
impl pane::web::WebTransport for FixtureTransport {
    fn get(
        &self,
        url: &str,
        _max_bytes: usize,
        _timeout: std::time::Duration,
    ) -> Result<pane::web::WebResponse, String> {
        Ok(pane::web::WebResponse {
            status: 200,
            location: None,
            content_type: "application/json".into(),
            body: if url.contains("/search?") {
                br#"{"results":[{"title":"Fixture","url":"https://example.com/page","content":"source excerpt"}]}"#.to_vec()
            } else {
                b"source content".to_vec()
            },
        })
    }
}

#[test]
fn native_cells_can_search_then_fetch_and_retain_source_provenance() {
    let root = std::env::current_dir().unwrap();
    let broker = pane::web::WebBroker::with_transport(
        pane::web::WebConfig {
            enabled: true,
            search_endpoint: Some("https://example.com/search".into()),
            ..Default::default()
        },
        Box::new(FixtureTransport),
    )
    .unwrap();
    let mut runtime = Runtime::new(
        &Profile::compile(&root, None),
        &Glasshouse::None,
        &SessionId::new("web-integrated"),
    )
    .with_web_broker(broker);
    let first = runtime.run_cell("const found = web.search('fixture'); const source = web.fetch(found.results[0].url); return source.content + ' ' + source.citation;");
    match first {
        CellOutcome::Returned { value, turn, .. } => {
            assert_eq!(
                value,
                Value::string("source content https://example.com/page")
            );
            let record = serde_json::to_string(&turn.record).unwrap();
            assert!(record.contains("web.search"));
            assert!(record.contains("web.fetch"));
        }
        other => panic!("expected web result, got {other:?}"),
    }
}
