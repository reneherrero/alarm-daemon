#![allow(clippy::panic, clippy::unwrap_used)]

//! Verifies that the static introspection schema in
//! `data/dbus/org.helm.AlarmDaemon.Control.xml` describes exactly the same
//! D-Bus surface as the live daemon's `Introspect()` reply.
//!
//! The static XML is the contract shipped to packagers, codegen tools, and
//! downstream client developers; this test is the CI guard that prevents the
//! Rust `#[interface]` impl from drifting out of sync with it.
//!
//! Comparison strategy: parse both documents, extract the
//! `org.helm.AlarmDaemon.Control` interface as `(methods + signals + args)`
//! tuples, and assert structural equality. Doc-comment annotations,
//! whitespace, and the auto-emitted standard interfaces (Introspectable,
//! Peer, Properties) are deliberately ignored — only the behavioural surface
//! has to match.

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

const TARGET_INTERFACE: &str = "org.helm.AlarmDaemon.Control";
const STATIC_XML: &str = "data/dbus/org.helm.AlarmDaemon.Control.xml";

#[test]
fn static_introspection_xml_matches_live_daemon() {
    let _env = common::TestEnv::acquire();
    common::require_tools();

    let install = r#"
set -euo pipefail
./setup.sh install
test -x "$HOME/.local/bin/alarm-daemon"
"#;
    assert!(
        common::run_bash(install),
        "setup install failed in introspect_xml test"
    );

    let live_xml = capture_live_introspection();
    let static_xml = std::fs::read_to_string(repo_root().join(STATIC_XML))
        .unwrap_or_else(|e| panic!("failed to read {STATIC_XML}: {e}"));

    let live = parse_interface(&live_xml, TARGET_INTERFACE)
        .unwrap_or_else(|| panic!("live introspection lacked {TARGET_INTERFACE}\nlive XML:\n{live_xml}"));
    let stat = parse_interface(&static_xml, TARGET_INTERFACE)
        .unwrap_or_else(|| panic!("static schema lacked {TARGET_INTERFACE}"));

    assert_eq!(
        stat, live,
        "\nstatic schema ({STATIC_XML}) drifted from the daemon's live introspection\n\
         (left = static, right = live).\n\
         If you intentionally changed the Rust #[interface], update the static \
         XML to match; otherwise revert the Rust change.\n\
         live XML follows for reference:\n{live_xml}"
    );
}

/// Boots a fresh daemon under `dbus-run-session` and returns the XML reply
/// from `Introspect()` on the control object. Uses the same install layout
/// as the rest of the integration suite.
fn capture_live_introspection() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| String::from("."));
    let script = format!(
        r#"
set -euo pipefail
dbus-run-session -- bash -lc '
  set -euo pipefail
  export HOME="{home}"
  DB_PATH="$(mktemp -u /tmp/alarm-daemon-introspect-XXXXXX.redb)"
  trap "rm -f \"${{DB_PATH}}\"" EXIT
  ALARM_DAEMON_BUS=session ALARM_DAEMON_DB_PATH="${{DB_PATH}}" \
  ALARM_DAEMON_SOUND_DIR="$HOME/.local/share/helm/sounds" \
  ALARM_DAEMON_CUSTOM_SOUND_DIR="$HOME/.local/share/helm/custom-sounds" \
  "$HOME/.local/bin/alarm-daemon" >/tmp/alarm-daemon-introspect.log 2>&1 &
  DAEMON_PID=$!
  trap "kill ${{DAEMON_PID}} 2>/dev/null || true; wait ${{DAEMON_PID}} 2>/dev/null || true; rm -f \"${{DB_PATH}}\"" EXIT
  for _ in $(seq 1 50); do
    if busctl --user call org.helm.AlarmDaemon /org/helm/AlarmDaemon \
        org.helm.AlarmDaemon.Control Status >/dev/null 2>&1; then
      break
    fi
    sleep 0.1
  done
  busctl --user --xml-interface introspect org.helm.AlarmDaemon \
    /org/helm/AlarmDaemon org.helm.AlarmDaemon.Control
'
"#,
        home = home
    );
    let output = Command::new("bash")
        .current_dir(repo_root())
        .arg("-lc")
        .arg(&script)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn dbus-run-session: {e}"));
    assert!(
        output.status.success(),
        "live introspection capture failed: status={:?}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap_or_else(|e| panic!("live introspection output is not valid UTF-8: {e}"))
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

// --------------------------------------------------------------------------
// Structural extraction.
// --------------------------------------------------------------------------

/// Extracted surface for a single D-Bus interface, suitable for `assert_eq!`.
///
/// Methods/signals are collected into `BTreeMap`s keyed by name so the
/// comparison is order-independent (D-Bus method order is not semantically
/// significant), but the `args` lists inside each entry preserve declaration
/// order because argument order absolutely *is* significant.
#[derive(Debug, PartialEq, Eq)]
struct InterfaceSurface {
    methods: BTreeMap<String, Vec<Arg>>,
    signals: BTreeMap<String, Vec<Arg>>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
struct Arg {
    name: Option<String>,
    signature: String,
    direction: Option<String>,
}

/// Parses `xml` and returns the structural surface for the named interface,
/// or `None` if no `<interface name="...">` element with that name was found.
fn parse_interface(xml: &str, target: &str) -> Option<InterfaceSurface> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut surface = InterfaceSurface {
        methods: BTreeMap::new(),
        signals: BTreeMap::new(),
    };

    // We track nesting state explicitly so an unrelated <interface> doesn't
    // contaminate our extraction.
    let mut in_target_iface = false;
    let mut current_member: Option<(MemberKind, String, Vec<Arg>)> = None;

    loop {
        match reader.read_event() {
            Err(e) => panic!(
                "introspection XML parse error at byte {}: {e}",
                reader.buffer_position()
            ),
            Ok(Event::Eof) => break,

            Ok(Event::Start(e)) => match e.name().as_ref() {
                b"interface" if attr(&e, "name").as_deref() == Some(target) => {
                    in_target_iface = true;
                }
                b"method" if in_target_iface => {
                    let name = attr(&e, "name").unwrap_or_default();
                    current_member = Some((MemberKind::Method, name, Vec::new()));
                }
                b"signal" if in_target_iface => {
                    let name = attr(&e, "name").unwrap_or_default();
                    current_member = Some((MemberKind::Signal, name, Vec::new()));
                }
                _ => {}
            },

            Ok(Event::Empty(e)) => match e.name().as_ref() {
                b"arg" if current_member.is_some() => {
                    if let Some((_, _, args)) = current_member.as_mut() {
                        args.push(Arg {
                            name: attr(&e, "name"),
                            signature: attr(&e, "type").unwrap_or_default(),
                            direction: attr(&e, "direction"),
                        });
                    }
                }
                b"method" | b"signal" if in_target_iface => {
                    // Self-closing method/signal with no args (e.g. <method name="Disarm"/>).
                    let kind = if e.name().as_ref() == b"method" {
                        MemberKind::Method
                    } else {
                        MemberKind::Signal
                    };
                    let name = attr(&e, "name").unwrap_or_default();
                    insert_member(&mut surface, kind, name, Vec::new());
                }
                _ => {}
            },

            Ok(Event::End(e)) => match e.name().as_ref() {
                b"interface" => {
                    in_target_iface = false;
                }
                b"method" | b"signal" if current_member.is_some() => {
                    if let Some((kind, name, args)) = current_member.take() {
                        insert_member(&mut surface, kind, name, args);
                    }
                }
                _ => {}
            },

            // Skip text, comments, doc annotations, processing instructions.
            _ => {}
        }
    }

    if surface.methods.is_empty() && surface.signals.is_empty() {
        None
    } else {
        Some(surface)
    }
}

#[derive(Debug, Clone, Copy)]
enum MemberKind {
    Method,
    Signal,
}

fn insert_member(
    surface: &mut InterfaceSurface,
    kind: MemberKind,
    name: String,
    args: Vec<Arg>,
) {
    match kind {
        MemberKind::Method => {
            surface.methods.insert(name, args);
        }
        MemberKind::Signal => {
            surface.signals.insert(name, args);
        }
    }
}

fn attr(e: &BytesStart<'_>, key: &str) -> Option<String> {
    e.attributes()
        .with_checks(false)
        .filter_map(Result::ok)
        .find(|a| a.key.as_ref() == key.as_bytes())
        .and_then(|a| a.unescape_value().ok().map(|v| v.into_owned()))
}

// --------------------------------------------------------------------------
// Pure unit tests for the parser (run without booting the daemon).
// --------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod parser_tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0"?>
<node>
  <interface name="org.example.Other">
    <method name="ShouldBeIgnored"/>
  </interface>
  <interface name="org.helm.AlarmDaemon.Control">
    <method name="Arm">
      <arg name="sound_id" type="s" direction="in"/>
    </method>
    <method name="Status">
      <arg type="b" direction="out"/>
    </method>
    <signal name="StateChanged">
      <arg name="armed" type="b"/>
    </signal>
  </interface>
</node>"#;

    #[test]
    fn extracts_only_the_target_interface() {
        let surface = parse_interface(SAMPLE, TARGET_INTERFACE).unwrap();
        assert_eq!(surface.methods.len(), 2);
        assert!(surface.methods.contains_key("Arm"));
        assert!(surface.methods.contains_key("Status"));
        assert!(!surface.methods.contains_key("ShouldBeIgnored"));
        assert_eq!(surface.signals.len(), 1);
    }

    #[test]
    fn captures_arg_attributes_in_declaration_order() {
        let surface = parse_interface(SAMPLE, TARGET_INTERFACE).unwrap();
        let arm_args = surface.methods.get("Arm").unwrap();
        assert_eq!(
            arm_args,
            &vec![Arg {
                name: Some("sound_id".to_owned()),
                signature: "s".to_owned(),
                direction: Some("in".to_owned()),
            }]
        );
        let status_args = surface.methods.get("Status").unwrap();
        assert_eq!(
            status_args,
            &vec![Arg {
                name: None,
                signature: "b".to_owned(),
                direction: Some("out".to_owned()),
            }]
        );
    }

    #[test]
    fn returns_none_when_target_interface_is_absent() {
        let xml = r#"<node><interface name="org.example.Foo"/></node>"#;
        assert!(parse_interface(xml, TARGET_INTERFACE).is_none());
    }

    #[test]
    fn ignores_doc_annotations_and_comments() {
        let xml = r#"<?xml version="1.0"?>
<node xmlns:doc="http://www.freedesktop.org/dbus/1.0/doc.dtd">
  <interface name="org.helm.AlarmDaemon.Control">
    <!-- This comment must not break parsing. -->
    <doc:doc><doc:description><doc:para>Ignored.</doc:para></doc:description></doc:doc>
    <method name="Disarm">
      <doc:doc><doc:description><doc:para>Ignored.</doc:para></doc:description></doc:doc>
    </method>
  </interface>
</node>"#;
        let surface = parse_interface(xml, TARGET_INTERFACE).unwrap();
        assert_eq!(surface.methods.len(), 1);
        assert_eq!(surface.methods.get("Disarm"), Some(&Vec::<Arg>::new()));
    }

    #[test]
    fn handles_self_closing_method_with_no_args() {
        let xml = r#"<node><interface name="org.helm.AlarmDaemon.Control">
            <method name="Disarm"/>
        </interface></node>"#;
        let surface = parse_interface(xml, TARGET_INTERFACE).unwrap();
        assert_eq!(surface.methods.get("Disarm"), Some(&Vec::<Arg>::new()));
    }
}
