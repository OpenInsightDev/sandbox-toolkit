//! WebDAV XML generation — multistatus responses, PROPFIND property rendering, active-lock XML.

use std::io::Cursor;
use std::time::UNIX_EPOCH;

use axum::{body::Body, http::StatusCode, response::Response};
use quick_xml::Writer;
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};

use super::{PropEntry, PropRequest};

/// WebDAV XML namespace URI. Used as the "D" prefix in all generated XML elements.
pub const DAV_NS: &str = "DAV:";

/// List of supported live properties for PROPFIND responses.
pub const SUPPORTED_PROPS: &[&str] = &[
    "creationdate",
    "getcontentlength",
    "getcontenttype",
    "getetag",
    "getlastmodified",
    "lockdiscovery",
    "resourcetype",
    "supportedlock",
];

/// Zero-sized struct holding all `D:`-prefixed XML element name constants.
///
/// ```
/// use sbx::webdav::xml::El;
/// assert_eq!(El::MULTI_STATUS, "D:multistatus");
/// assert_eq!(El::PROP, "D:prop");
/// ```
pub struct El;

impl El {
    // ── Shared (pub) element names ──────────────────────────────
    pub const ACTIVE_LOCK: &str = "D:activelock";
    pub const COLLECTION: &str = "D:collection";
    pub const DEPTH: &str = "D:depth";
    pub const EXCLUSIVE: &str = "D:exclusive";
    pub const HREF: &str = "D:href";
    pub const LOCK_DISCOVERY: &str = "D:lockdiscovery";
    pub const LOCK_ENTRY: &str = "D:lockentry";
    pub const LOCK_SCOPE: &str = "D:lockscope";
    pub const LOCK_TOKEN: &str = "D:locktoken";
    pub const LOCK_TYPE: &str = "D:locktype";
    pub const MULTI_STATUS: &str = "D:multistatus";
    pub const OWNER: &str = "D:owner";
    pub const PROP: &str = "D:prop";
    pub const PROP_STAT: &str = "D:propstat";
    pub const RESOURCE_TYPE: &str = "D:resourcetype";
    pub const RESPONSE: &str = "D:response";
    pub const SHARED: &str = "D:shared";
    pub const STATUS: &str = "D:status";
    pub const SUPPORTED_LOCK: &str = "D:supportedlock";
    pub const TIMEOUT: &str = "D:timeout";
    pub const WRITE: &str = "D:write";

    // ── Live property element names (crate-only) ───────────────
    pub(crate) const CREATION_DATE: &str = "D:creationdate";
    pub(crate) const GET_CONTENT_LENGTH: &str = "D:getcontentlength";
    pub(crate) const GET_CONTENT_TYPE: &str = "D:getcontenttype";
    pub(crate) const GET_ETAG: &str = "D:getetag";
    pub(crate) const GET_LAST_MODIFIED: &str = "D:getlastmodified";
}

/// Convenience alias for the XML writer used throughout `sbx`.
///
/// `Writer<Cursor<Vec<u8>>>` backed by an in-memory byte buffer.
pub type XmlWriter = Writer<Cursor<Vec<u8>>>;

/// Extension trait that provides a shorthand for writing XML events.
///
/// The `.ev(event)` method is equivalent to `.write_event(event).unwrap()`,
/// reducing boilerplate in WebDAV response building.
///
/// # Panics
///
/// Panics if the underlying XML writer fails to write the event.
/// In normal operation this never occurs — the backing buffer is an
/// in-memory `Vec<u8>` which is infallible.
///
/// ```
/// use std::io::Cursor;
/// use quick_xml::{Writer, events::{BytesStart, BytesEnd, Event}};
/// use sbx::webdav::xml::{XmlWriter, XmlWriterExt, El};
///
/// let mut w = Writer::new(Cursor::new(Vec::new()));
/// w.ev(Event::Start(BytesStart::new(El::RESPONSE)));
/// w.ev(Event::End(BytesEnd::new(El::RESPONSE)));
/// let xml = String::from_utf8(w.into_inner().into_inner()).unwrap();
/// assert!(xml.contains("D:response"));
/// ```
pub trait XmlWriterExt {
    fn ev(&mut self, event: Event<'_>);
}

impl XmlWriterExt for XmlWriter {
    fn ev(&mut self, event: Event<'_>) {
        self.write_event(event).unwrap();
    }
}

/// Build a `207 Multi-Status` XML response.
///
/// # Panics
///
/// Panics if the response builder fails to construct a valid response.
/// This only occurs when the builder is in an invalid state (e.g. body
/// already set), which cannot happen with a fresh builder.
///
/// ```
/// use sbx::webdav::xml::multistatus;
///
/// let response = multistatus("<D:multistatus xmlns:D='DAV:'/>".into());
/// assert_eq!(response.status().as_u16(), 207);
/// assert!(response.headers().get("content-type").unwrap().to_str().unwrap().contains("application/xml"));
/// ```
pub fn multistatus(xml: String) -> Response {
    Response::builder()
        .status(StatusCode::MULTI_STATUS) // 207 Multi-Status
        .header("content-type", "application/xml; charset=utf-8")
        .body(Body::from(xml))
        .unwrap()
}

/// Build a full `multistatus` XML body from a list of `PropEntry`s
/// and the corresponding `PropRequest`.
///
/// This is the primary XML serialization function for PROPFIND responses.
/// For each entry it emits the requested live properties (creationdate,
/// getcontentlength, getetag, resourcetype, etc.), dead properties, and
/// active lock information via [`write_activelock`].
///
/// # Panics
///
/// Panics if the assembled XML buffer contains invalid UTF-8.
/// In normal operation this never occurs — `quick_xml` always produces
/// valid UTF-8 when writing to an in-memory buffer.
///
/// ```
/// use std::time::{SystemTime, UNIX_EPOCH};
/// use sbx::webdav::{PropEntry, PropRequest, Depth};
/// use sbx::webdav::xml::build_multistatus;
///
/// let entry = PropEntry {
///     href: "/file.txt".into(),
///     modified: UNIX_EPOCH,
///     created: None,
///     size: 42,
///     is_dir: false,
///     content_type: Some("text/plain".into()),
///     dead_props: None,
///     active_locks: None,
///     canonical_path: None,
/// };
/// let xml = build_multistatus(&[entry], &PropRequest::AllProp);
/// assert!(xml.contains("D:multistatus"));
/// assert!(xml.contains("D:href>/file.txt</D:href>"));
/// ```
pub fn build_multistatus(entries: &[PropEntry], prop_request: &PropRequest) -> String {
    let mut writer = Writer::new(Cursor::new(Vec::with_capacity(entries.len() * 700)));

    writer.ev(Event::Decl(BytesDecl::new("1.0", Some("utf-8"), None)));

    let mut ms = BytesStart::new(El::MULTI_STATUS);
    ms.push_attribute(("xmlns:D", DAV_NS));
    writer.ev(Event::Start(ms));

    for entry in entries {
        write_response(&mut writer, entry, prop_request);
    }

    writer.ev(Event::End(BytesEnd::new(El::MULTI_STATUS)));

    String::from_utf8(writer.into_inner().into_inner()).unwrap()
}

fn write_response(writer: &mut XmlWriter, entry: &PropEntry, prop_request: &PropRequest) {
    writer.ev(Event::Start(BytesStart::new(El::RESPONSE)));

    writer.ev(Event::Start(BytesStart::new(El::HREF)));
    writer.ev(Event::Text(BytesText::new(&entry.href)));
    writer.ev(Event::End(BytesEnd::new(El::HREF)));

    if matches!(prop_request, PropRequest::PropName) {
        write_propname(writer, SUPPORTED_PROPS);
        writer.ev(Event::End(BytesEnd::new(El::RESPONSE)));
        return;
    }

    let (found, missing) = match prop_request {
        PropRequest::AllProp => (SUPPORTED_PROPS.to_vec(), vec![]),
        PropRequest::Named(names) => {
            let (mut found, mut missing) = (Vec::new(), Vec::new());

            for n in names {
                let local = super::parse_clark(n).map(|(_, l)| l).unwrap_or(n.as_str());
                if SUPPORTED_PROPS.contains(&local) {
                    if !found.contains(&local) {
                        found.push(local);
                    }
                } else {
                    missing.push(n.as_str());
                }
            }

            (found, missing)
        }
        PropRequest::PropName => unreachable!(),
    };

    let applicable = found
        .iter()
        .filter(|p| is_applicable(p, entry.is_dir))
        .copied();
    let mut applicable = applicable.peekable();

    if applicable.peek().is_some() {
        write_propstat_200(writer, entry, applicable);
    }

    let mut not_found: Vec<String> = found
        .iter()
        .filter(|p| !is_applicable(p, entry.is_dir))
        .map(|p| p.to_string())
        .collect();
    not_found.extend(missing.iter().map(|p| p.to_string()));

    // Exclude properties that have dead-prop values — they appear in the
    // 200 dead-propstat below, not here in the 404 section.
    if let Some(ref dead) = entry.dead_props {
        not_found.retain(|n| !dead.contains_key(n));
    }

    if !not_found.is_empty() {
        write_propstat_404(writer, &not_found);
    }

    // Dead properties
    if let Some(ref dead) = entry.dead_props
        && !dead.is_empty()
    {
        write_dead_propstat(writer, dead);
    }

    writer.ev(Event::End(BytesEnd::new(El::RESPONSE)));
}

fn is_applicable(prop: &str, is_dir: bool) -> bool {
    match prop {
        "getcontentlength" | "getcontenttype" | "getetag" => !is_dir,
        _ => true,
    }
}

fn write_prop_text(writer: &mut XmlWriter, qname: &str, value: &str) {
    writer.ev(Event::Start(BytesStart::new(qname)));
    writer.ev(Event::Text(BytesText::new(value)));
    writer.ev(Event::End(BytesEnd::new(qname)));
}

fn write_propstat_200<'a, I>(writer: &mut XmlWriter, entry: &PropEntry, props: I)
where
    I: Iterator<Item = &'a str>,
{
    writer.ev(Event::Start(BytesStart::new(El::PROP_STAT)));
    writer.ev(Event::Start(BytesStart::new(El::PROP)));

    for prop_name in props {
        match prop_name {
            "creationdate" => {
                let date = entry
                    .created
                    .map(crate::utils::time::format_rfc3339)
                    .unwrap_or_default();
                write_prop_text(writer, El::CREATION_DATE, &date);
            }
            "getcontentlength" => {
                write_prop_text(writer, El::GET_CONTENT_LENGTH, &entry.size.to_string());
            }
            "getcontenttype" => {
                write_prop_text(
                    writer,
                    El::GET_CONTENT_TYPE,
                    entry.content_type.as_deref().unwrap_or(""),
                );
            }
            "getetag" => {
                let etag = format!(
                    "{:x}-{:x}",
                    entry
                        .modified
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                    entry.size
                );
                write_prop_text(writer, El::GET_ETAG, &etag);
            }
            "getlastmodified" => {
                let date = crate::utils::time::format_rfc1123(entry.modified);
                write_prop_text(writer, El::GET_LAST_MODIFIED, &date);
            }
            "lockdiscovery" => {
                writer.ev(Event::Start(BytesStart::new(El::LOCK_DISCOVERY)));
                if let Some(ref locks) = entry.active_locks {
                    for lock in locks {
                        write_activelock(writer, lock);
                    }
                }
                writer.ev(Event::End(BytesEnd::new(El::LOCK_DISCOVERY)));
            }
            "resourcetype" => {
                writer.ev(Event::Start(BytesStart::new(El::RESOURCE_TYPE)));
                if entry.is_dir {
                    writer.ev(Event::Empty(BytesStart::new(El::COLLECTION)));
                }
                writer.ev(Event::End(BytesEnd::new(El::RESOURCE_TYPE)));
            }
            "supportedlock" => {
                writer.ev(Event::Start(BytesStart::new(El::SUPPORTED_LOCK)));
                write_lockentry(writer, "exclusive");
                write_lockentry(writer, "shared");
                writer.ev(Event::End(BytesEnd::new(El::SUPPORTED_LOCK)));
            }
            _ => {}
        }
    }

    writer.ev(Event::End(BytesEnd::new(El::PROP)));
    writer.ev(Event::Start(BytesStart::new(El::STATUS)));
    writer.ev(Event::Text(BytesText::new("HTTP/1.1 200 OK")));
    writer.ev(Event::End(BytesEnd::new(El::STATUS)));
    writer.ev(Event::End(BytesEnd::new(El::PROP_STAT)));
}

fn write_lockentry(writer: &mut XmlWriter, scope: &str) {
    writer.ev(Event::Start(BytesStart::new(El::LOCK_ENTRY)));
    writer.ev(Event::Start(BytesStart::new(El::LOCK_SCOPE)));

    if scope == "exclusive" {
        writer.ev(Event::Empty(BytesStart::new(El::EXCLUSIVE)));
    } else {
        writer.ev(Event::Empty(BytesStart::new(El::SHARED)));
    }

    writer.ev(Event::End(BytesEnd::new(El::LOCK_SCOPE)));
    writer.ev(Event::Start(BytesStart::new(El::LOCK_TYPE)));
    writer.ev(Event::Empty(BytesStart::new(El::WRITE)));
    writer.ev(Event::End(BytesEnd::new(El::LOCK_TYPE)));
    writer.ev(Event::End(BytesEnd::new(El::LOCK_ENTRY)));
}

fn write_propstat_404(writer: &mut XmlWriter, props: &[String]) {
    writer.ev(Event::Start(BytesStart::new(El::PROP_STAT)));
    writer.ev(Event::Start(BytesStart::new(El::PROP)));

    for prop_name in props {
        let (ns, local) = super::parse_clark(prop_name).unwrap_or(("", prop_name));
        let mut elem = BytesStart::new(local);
        if !ns.is_empty() {
            elem.push_attribute(("xmlns", ns));
        }
        writer.ev(Event::Empty(elem));
    }

    writer.ev(Event::End(BytesEnd::new(El::PROP)));
    writer.ev(Event::Start(BytesStart::new(El::STATUS)));
    writer.ev(Event::Text(BytesText::new("HTTP/1.1 404 Not Found")));
    writer.ev(Event::End(BytesEnd::new(El::STATUS)));
    writer.ev(Event::End(BytesEnd::new(El::PROP_STAT)));
}

fn write_propname(writer: &mut XmlWriter, props: &[&str]) {
    writer.ev(Event::Start(BytesStart::new(El::PROP_STAT)));
    writer.ev(Event::Start(BytesStart::new(El::PROP)));

    for prop_name in props {
        writer.ev(Event::Empty(BytesStart::new(format!("D:{prop_name}"))));
    }

    writer.ev(Event::End(BytesEnd::new(El::PROP)));
    writer.ev(Event::Start(BytesStart::new(El::STATUS)));
    writer.ev(Event::Text(BytesText::new("HTTP/1.1 200 OK")));
    writer.ev(Event::End(BytesEnd::new(El::STATUS)));
    writer.ev(Event::End(BytesEnd::new(El::PROP_STAT)));
}

/// Write an `<D:activelock>` element for a lock.
///
/// Produces the full active-lock XML subtree including lockscope, locktype,
/// depth, owner, timeout, and locktoken. Used by both the LOCK response
/// handler and the PROPFIND `lockdiscovery` property.
///
/// ```
/// use std::time::{SystemTime, Duration};
/// use std::io::Cursor;
/// use quick_xml::Writer;
/// use sbx::webdav::{LockInfo, LockScope, Depth};
/// use sbx::webdav::xml::{XmlWriter, XmlWriterExt, write_activelock};
///
/// let lock = LockInfo {
///     scope: LockScope::Exclusive,
///     token: "opaquelocktoken:abc".into(),
///     owner: Some("user".into()),
///     created: SystemTime::now(),
///     timeout: Some(Duration::from_secs(3600)),
///     depth: Depth::Zero,
/// };
/// let mut w = Writer::new(Cursor::new(Vec::new()));
/// write_activelock(&mut w, &lock);
/// let xml = String::from_utf8(w.into_inner().into_inner()).unwrap();
/// assert!(xml.contains("D:lockscope"));
/// assert!(xml.contains("D:exclusive"));
/// assert!(xml.contains("opaquelocktoken:abc"));
/// ```
pub fn write_activelock(writer: &mut XmlWriter, lock: &super::LockInfo) {
    writer.ev(Event::Start(BytesStart::new(El::ACTIVE_LOCK)));

    writer.ev(Event::Start(BytesStart::new(El::LOCK_SCOPE)));
    match lock.scope {
        super::LockScope::Exclusive => {
            writer.ev(Event::Empty(BytesStart::new(El::EXCLUSIVE)));
        }
        super::LockScope::Shared => {
            writer.ev(Event::Empty(BytesStart::new(El::SHARED)));
        }
    }
    writer.ev(Event::End(BytesEnd::new(El::LOCK_SCOPE)));

    writer.ev(Event::Start(BytesStart::new(El::LOCK_TYPE)));
    writer.ev(Event::Empty(BytesStart::new(El::WRITE)));
    writer.ev(Event::End(BytesEnd::new(El::LOCK_TYPE)));

    let depth_str = match lock.depth {
        super::Depth::Zero => "0",
        super::Depth::One => "1",
        super::Depth::Infinity => "infinity",
    };
    writer.ev(Event::Start(BytesStart::new(El::DEPTH)));
    writer.ev(Event::Text(BytesText::new(depth_str)));
    writer.ev(Event::End(BytesEnd::new(El::DEPTH)));

    if let Some(ref owner) = lock.owner {
        writer.ev(Event::Start(BytesStart::new(El::OWNER)));
        writer.ev(Event::Text(BytesText::new(owner)));
        writer.ev(Event::End(BytesEnd::new(El::OWNER)));
    }

    if let Some(d) = lock.timeout {
        writer.ev(Event::Start(BytesStart::new(El::TIMEOUT)));
        writer.ev(Event::Text(BytesText::new(&format!(
            "Second-{}",
            d.as_secs()
        ))));
        writer.ev(Event::End(BytesEnd::new(El::TIMEOUT)));
    }

    writer.ev(Event::Start(BytesStart::new(El::LOCK_TOKEN)));
    writer.ev(Event::Start(BytesStart::new(El::HREF)));
    writer.ev(Event::Text(BytesText::new(&lock.token)));
    writer.ev(Event::End(BytesEnd::new(El::HREF)));
    writer.ev(Event::End(BytesEnd::new(El::LOCK_TOKEN)));

    writer.ev(Event::End(BytesEnd::new(El::ACTIVE_LOCK)));
}

fn write_dead_propstat(writer: &mut XmlWriter, props: &std::collections::HashMap<String, String>) {
    writer.ev(Event::Start(BytesStart::new(El::PROP_STAT)));
    writer.ev(Event::Start(BytesStart::new(El::PROP)));

    for (clark_key, value) in props {
        let (ns, local) = super::parse_clark(clark_key).unwrap_or(("", clark_key));
        let mut elem = BytesStart::new(local);
        if !ns.is_empty() {
            elem.push_attribute(("xmlns", ns));
        }
        writer.ev(Event::Start(elem));
        writer.ev(Event::Text(BytesText::new(value)));
        writer.ev(Event::End(BytesEnd::new(local)));
    }

    writer.ev(Event::End(BytesEnd::new(El::PROP)));
    writer.ev(Event::Start(BytesStart::new(El::STATUS)));
    writer.ev(Event::Text(BytesText::new("HTTP/1.1 200 OK")));
    writer.ev(Event::End(BytesEnd::new(El::STATUS)));
    writer.ev(Event::End(BytesEnd::new(El::PROP_STAT)));
}
