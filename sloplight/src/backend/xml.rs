//! Minimal helpers for the XML documents GameStream-style hosts return.

use super::http::HttpError;

/// Returns the text of the first element named `tag`, or `None` if absent.
pub fn get_string(xml: &str, tag: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    doc.descendants()
        .find(|n| n.is_element() && n.tag_name().name() == tag)
        .map(|n| n.text().unwrap_or("").to_string())
}

/// Returns the text of every element named `tag`, in document order.
pub fn get_strings(xml: &str, tag: &str) -> Vec<String> {
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return Vec::new();
    };
    doc.descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == tag)
        .map(|n| n.text().unwrap_or("").to_string())
        .collect()
}

pub fn get_hex(xml: &str, tag: &str) -> Vec<u8> {
    get_string(xml, tag)
        .and_then(|s| hex::decode(s.trim()).ok())
        .unwrap_or_default()
}

/// Checks the `status_code` attribute of the `<root>` element.
pub fn verify_status(xml: &str) -> Result<(), HttpError> {
    let doc = roxmltree::Document::parse(xml)
        .map_err(|_| HttpError::Status { code: -1, message: "Malformed XML".into() })?;
    let root = doc.root_element();
    if root.tag_name().name() != "root" {
        return Err(HttpError::Status { code: -1, message: "Malformed XML (missing root element)".into() });
    }
    // GFE 3.20.3 can send 0xFFFFFFFF, so parse as unsigned and cast like upstream does.
    let code = root
        .attribute("status_code")
        .and_then(|c| c.parse::<u32>().ok())
        .map(|c| c as i32)
        .unwrap_or(-1);
    if code == 200 {
        return Ok(());
    }
    let mut message = root.attribute("status_message").unwrap_or("").to_string();
    let mut code = code;
    if code == -1 && message == "Invalid" {
        // Audio capture error that GFE doesn't describe
        code = 418;
        message = "Missing audio capture device. Reinstalling GeForce Experience should resolve this error.".into();
    }
    Err(HttpError::Status { code, message })
}

/// Parses the root status code of an XML body, if it has one. Used to
/// turn HTTP error responses into protocol errors when possible.
pub fn root_status(xml: &str) -> Option<(i32, String)> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let root = doc.root_element();
    if root.tag_name().name() != "root" {
        return None;
    }
    let code = root.attribute("status_code")?.parse::<u32>().ok()? as i32;
    Some((code, root.attribute("status_message").unwrap_or("").to_string()))
}
