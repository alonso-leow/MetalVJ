//! D2 (show-control lote) — **GDTF import**: a `.gdtf` fixture type becomes a
//! `data/fixtures/<model>.toml` profile in the existing [`crate::fixture`]
//! `Channel`/`Limits` shape.
//!
//! A `.gdtf` is a zip of XML and geometry: `description.xml` at its root is
//! the entry point, `<FixtureType>` → `<DMXModes>` → `<DMXMode>` →
//! `<DMXChannels>` → `<DMXChannel>` (an `Offset`, and its channels) →
//! `<LogicalChannel>` (an `Attribute`) → `<ChannelFunction>`
//! (`DMXFrom`/`DMXTo`, `PhysicalFrom`/`PhysicalTo`). The mapping the report
//! (§5, grandMA3 via `libMVRgdtf`) fixes: a mode is a numbered channel list,
//! each DMX channel has an offset, a function maps a value to a raw byte
//! range `DMXFrom..DMXTo`, two bytes are one 16-bit value (`Coarse`/`Fine`).
//!
//! # What is imported, and what is not
//!
//! - `Pan`/`Tilt`/`Dimmer`/beam (`Zoom`/`Focus`/`Iris`/`Frost`)/colour
//!   (`Hue`/`Saturation`, and RGB/`ColorAdd_*` onto the two spare selectors
//!   `a`/`b`) map onto an existing [`Selector`]; anything with no selector
//!   (a gobo wheel, a third colour byte) is skipped rather than invented —
//!   MetalVJ's row has thirteen floats, not a second channel model.
//! - `DMXFrom..DMXTo` becomes the channel's `physical` byte range; a function
//!   that spans two bytes (`…/2`) becomes coarse + fine (`Offset` and the
//!   slot after it).
//! - `PhysicalFrom`/`PhysicalTo` on the Pan/Tilt function become the profile's
//!   [`Limits`] travel in degrees.
//! - **Not** imported: the geometry/3DS (MetalVJ has no 3D preview of a
//!   fixture), multiple `DMXBreak`s, sub-channel `Relations`, and
//!   `InterpolationTo` shaping (the profile's remap is linear — a non-linear
//!   curve is imported as its endpoints).
//!
//! The importer never crashes on a malformed file: every failure is a
//! `Result::Err(String)` the caller turns into a 400, the same "a missing or
//! malformed profile is a skip, never a crash" rule `fixture.rs` states.
//!
//! # Reading the zip without a zip crate
//!
//! No `zip` crate is in the workspace, so [`unzip`] walks the central
//! directory and the local file headers itself (the report's own
//! "implement the minimal local-file-header reader"). A stored entry
//! (method 0) is copied; a DEFLATE one (method 8, what a real `.gdtf` uses)
//! goes through `flate2`, which is already in the tree. `description.xml`
//! itself is parsed by a small hand reader ([`xml_parse`]) — GDTF is a
//! well-formed subset, and no XML crate is a direct dependency either.

use std::path::Path;

use crate::fixture::{self, Channel, Fixture, Limits, Selector, WireKind};

/// Imports a `.gdtf` file at `path`. `name` overrides the profile's model id
/// (the file stem); `mode` picks a DMX mode by name or index (the first when
/// absent).
pub fn import_path(path: &Path, name: Option<&str>, mode: Option<&str>) -> Result<Fixture, String> {
    let bytes = metal_vfs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    import_bytes(&bytes, name, mode)
}

/// Imports a `.gdtf` from its bytes. See [`import_path`].
pub fn import_bytes(bytes: &[u8], name: Option<&str>, mode: Option<&str>) -> Result<Fixture, String> {
    let files = unzip(bytes)?;
    let xml = files
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("description.xml"))
        .or_else(|| files.iter().find(|(n, _)| n.to_ascii_lowercase().ends_with("description.xml")))
        .ok_or("the .gdtf has no description.xml")?;
    let text = std::str::from_utf8(&xml.1)
        .map_err(|e| format!("description.xml is not UTF-8: {e}"))?;
    from_description_xml(text, name, mode)
}

/// Parses one `description.xml` into a profile. Split out so a test can feed
/// the XML directly; [`import_bytes`] is the zip half.
pub fn from_description_xml(text: &str, name: Option<&str>, mode: Option<&str>) -> Result<Fixture, String> {
    let root = xml_parse(text)?;
    let ft = root
        .children
        .iter()
        .find(|c| c.name == "FixtureType")
        .or_else(|| (root.name == "FixtureType").then_some(&root))
        .ok_or("description.xml has no <FixtureType>")?;

    let label = ft
        .attr("Name")
        .or_else(|| ft.attr("ShortName"))
        .unwrap_or("GDTF fixture")
        .to_string();
    let model = sanitize_model(
        name.or_else(|| ft.attr("ShortName"))
            .or_else(|| ft.attr("Name"))
            .unwrap_or("gdtf-fixture"),
    );

    let dmx_modes = ft.child("DMXModes").ok_or("the fixture type has no <DMXModes>")?;
    let modes: Vec<&Xml> = dmx_modes.kids("DMXMode").collect();
    if modes.is_empty() {
        return Err("the fixture type has no <DMXMode>".into());
    }
    let mode_el = match mode {
        Some(m) => modes
            .iter()
            .find(|x| x.attr("Name") == Some(m))
            .or_else(|| m.parse::<usize>().ok().and_then(|i| modes.get(i)))
            .copied()
            .ok_or_else(|| {
                let have: Vec<&str> = modes.iter().filter_map(|x| x.attr("Name")).collect();
                format!("no DMX mode {m:?} (have: {})", have.join(", "))
            })?,
        None => modes[0],
    };
    let mode_name = mode_el.attr("Name").unwrap_or("");

    let mut channels: Vec<Channel> = Vec::new();
    let mut used: Vec<Selector> = Vec::new();
    let mut limits = Limits::default();
    if let Some(chs) = mode_el.child("DMXChannels") {
        for dc in chs.kids("DMXChannel") {
            let Some(offset) = dc
                .attr("Offset")
                .and_then(|v| v.trim().parse::<u16>().ok())
                .filter(|v| *v >= 1)
            else {
                continue;
            };
            for (attr, func) in channel_functions(dc) {
                let Some(signal) = selector_for(attr) else {
                    continue;
                };
                if used.contains(&signal) {
                    continue;
                }
                let (from, to, bytes) = function_range(func);
                let coarse = offset - 1;
                let fine = (bytes >= 2).then_some(offset);
                if let Some((pf, pt)) = physical_range(func) {
                    match signal {
                        Selector::Pan => limits.pan = [pf, pt],
                        Selector::Tilt => limits.tilt = [pf, pt],
                        _ => {}
                    }
                }
                used.push(signal);
                channels.push(Channel {
                    signal,
                    coarse: Some(coarse),
                    fine,
                    range: None,
                    physical: Some([from, to]),
                });
            }
        }
    }

    if channels.is_empty() {
        return Err(format!(
            "mode {mode_name:?} has no channel MetalVJ can map (Pan/Tilt/Dimmer/beam/colour)"
        ));
    }

    let fixture = Fixture {
        model,
        label,
        wire: WireKind::Dmx,
        bluetooth: None,
        universe: 0,
        address: 1,
        channels,
        limits,
    };
    // The one validator: a profile this returns is always one `parse` reads.
    fixture::parse(&fixture.model, &fixture.to_toml()?)?;
    Ok(fixture)
}

/// Writes `f` to `data/fixtures/<model>.toml` under `root`, creating the
/// directory — the write side of an import, the counterpart of [`fixture::load`].
pub fn save(root: &Path, f: &Fixture) -> Result<(), String> {
    let dir = fixture::dir(root);
    metal_vfs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let text = f.to_toml()?;
    crate::atomic::write(dir.join(format!("{}.toml", f.model)), text).map_err(|e| e.to_string())
}

/// The function to use for each attribute a DMX channel carries: the logical
/// channel's own attribute first, then a bare `<ChannelFunction>` (some
/// minimal fixtures skip the wrapper). Returns `(attribute, function)` pairs.
fn channel_functions<'a>(dc: &'a Xml) -> Vec<(&'a str, &'a Xml)> {
    let mut out = Vec::new();
    for lc in dc.kids("LogicalChannel") {
        let attr = lc.attr("Attribute").unwrap_or("");
        if let Some(func) = lc
            .kids("ChannelFunction")
            .find(|f| f.attr("Attribute") == Some(attr))
            .or_else(|| lc.child("ChannelFunction"))
        {
            let attr = if attr.is_empty() { func.attr("Attribute").unwrap_or("") } else { attr };
            out.push((attr, func));
        }
    }
    if out.is_empty() {
        for f in dc.kids("ChannelFunction") {
            out.push((f.attr("Attribute").unwrap_or(""), f));
        }
    }
    out
}

/// Which row float a GDTF attribute drives, if any. Unknown attributes (a
/// gobo wheel, a third colour byte) return `None` and are skipped.
fn selector_for(attr: &str) -> Option<Selector> {
    Some(match attr.trim() {
        "Pan" => Selector::Pan,
        "Tilt" => Selector::Tilt,
        "Dimmer" | "Intensity" | "Master" | "ShutterDimmer" => Selector::Level,
        "Strobe" | "Shutter" | "StrobeFrequency" | "ShutterStrobeSlowFast" => Selector::Strobe,
        "Zoom" | "Zoom1" => Selector::Zoom,
        "Focus" | "Focus1" => Selector::Focus,
        "Iris" => Selector::Iris,
        "Frost" | "Frost1" | "Frost2" => Selector::Frost,
        "ColorTemperature" | "CTO" | "CTC" | "CTB" => Selector::Cct,
        "Hue" => Selector::Hue,
        "Saturation" => Selector::Sat,
        "ColorAdd_R" | "ColorRGB_Red" | "ColorAdd_R1" => Selector::A,
        "ColorAdd_G" | "ColorRGB_Green" | "ColorAdd_G1" => Selector::B,
        _ => return None,
    })
}

/// `DMXFrom`/`DMXTo`, each written `value/bytes` — the raw byte range and how
/// many bytes the value spans (2 = 16-bit). Defaults to a full 8-bit range.
fn function_range(f: &Xml) -> (f32, f32, u32) {
    let (from, fb) = dmx_value(f.attr("DMXFrom"));
    let (to, tb) = dmx_value(f.attr("DMXTo"));
    (from, to, fb.max(tb).max(1))
}

fn dmx_value(v: Option<&str>) -> (f32, u32) {
    let Some(v) = v else { return (0.0, 1) };
    let mut it = v.splitn(2, '/');
    let value = it.next().unwrap_or("").trim().parse::<f32>().unwrap_or(0.0);
    let bytes = it.next().and_then(|b| b.trim().parse::<u32>().ok()).unwrap_or(1);
    (value, bytes)
}

fn physical_range(f: &Xml) -> Option<(f32, f32)> {
    let from = f.attr("PhysicalFrom")?.trim().parse::<f32>().ok()?;
    let to = f.attr("PhysicalTo")?.trim().parse::<f32>().ok()?;
    Some((from, to))
}

/// A model id that is a usable file stem: lowercase, runs of separators to
/// one `-`, anything else dropped. Never empty.
fn sanitize_model(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "gdtf-fixture".to_string()
    } else {
        out
    }
}

// ---------------------------------------------------------------------------
// zip: central directory + local file headers
// ---------------------------------------------------------------------------

fn le16(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(o)?, *b.get(o + 1)?]))
}

fn le32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *b.get(o)?,
        *b.get(o + 1)?,
        *b.get(o + 2)?,
        *b.get(o + 3)?,
    ]))
}

fn find_eocd(b: &[u8]) -> Option<usize> {
    if b.len() < 22 {
        return None;
    }
    let stop = b.len().saturating_sub(22 + 65535);
    let mut i = b.len() - 22;
    loop {
        if le32(b, i) == Some(0x0605_4b50) {
            return Some(i);
        }
        if i == stop {
            return None;
        }
        i -= 1;
    }
}

/// Every entry of the zip as `(name, bytes)`. Stored entries are copied,
/// DEFLATE ones inflated; any other method is refused by name.
pub fn unzip(b: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let eocd = find_eocd(b).ok_or("not a zip: no end-of-central-directory record")?;
    let count = le16(b, eocd + 10).ok_or("truncated zip directory")? as usize;
    let cd = le32(b, eocd + 16).ok_or("truncated zip directory")? as usize;
    let mut p = cd;
    let mut out = Vec::new();
    for _ in 0..count {
        if le32(b, p) != Some(0x0201_4b50) {
            break;
        }
        let method = le16(b, p + 10).unwrap_or(0);
        let comp = le32(b, p + 20).unwrap_or(0) as usize;
        let nlen = le16(b, p + 28).unwrap_or(0) as usize;
        let elen = le16(b, p + 30).unwrap_or(0) as usize;
        let clen = le16(b, p + 32).unwrap_or(0) as usize;
        let local = le32(b, p + 42).unwrap_or(0) as usize;
        let name = String::from_utf8_lossy(b.get(p + 46..p + 46 + nlen).unwrap_or(&[])).to_string();
        p += 46 + nlen + elen + clen;

        let lnlen = le16(b, local + 26).unwrap_or(0) as usize;
        let lelen = le16(b, local + 28).unwrap_or(0) as usize;
        let start = local + 30 + lnlen + lelen;
        let raw = b
            .get(start..start + comp)
            .ok_or_else(|| format!("zip entry {name:?} runs past the end"))?;
        let data = match method {
            0 => raw.to_vec(),
            8 => inflate(raw).map_err(|e| format!("zip entry {name:?}: {e}"))?,
            m => return Err(format!("zip entry {name:?}: unsupported compression {m}")),
        };
        out.push((name, data));
    }
    Ok(out)
}

fn inflate(raw: &[u8]) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut out = Vec::new();
    flate2::read::DeflateDecoder::new(raw)
        .read_to_end(&mut out)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// a small XML reader: elements, attributes, nesting; text is ignored
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
struct Xml {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Xml>,
}

impl Xml {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    fn child(&self, name: &str) -> Option<&Xml> {
        self.children.iter().find(|c| c.name == name)
    }

    fn kids<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Xml> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }
}

struct Reader<'a> {
    s: &'a str,
    b: &'a [u8],
    i: usize,
}

impl<'a> Reader<'a> {
    fn new(s: &'a str) -> Self {
        Reader { s, b: s.as_bytes(), i: 0 }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn at(&self, pat: &str) -> bool {
        self.s[self.i..].starts_with(pat)
    }

    fn bump(&mut self) -> Option<u8> {
        let c = self.peek();
        if c.is_some() {
            self.i += 1;
        }
        c
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.i += 1;
        }
    }

    /// Slices up to (not including) any byte of `delim`. ASCII delimiters
    /// never occur inside a UTF-8 continuation byte, so this cannot split one.
    fn until(&mut self, delim: &[u8]) -> &'a str {
        let start = self.i;
        while let Some(c) = self.peek() {
            if delim.contains(&c) {
                break;
            }
            self.i += 1;
        }
        &self.s[start..self.i]
    }

    fn skip_to(&mut self, pat: &str) {
        match self.s[self.i..].find(pat) {
            Some(rel) => self.i += rel + pat.len(),
            None => self.i = self.b.len(),
        }
    }
}

fn xml_parse(text: &str) -> Result<Xml, String> {
    let mut r = Reader::new(text);
    loop {
        r.skip_ws();
        if r.at("<?") {
            r.skip_to("?>");
        } else if r.at("<!--") {
            r.skip_to("-->");
        } else if r.at("<!") {
            r.skip_to(">");
        } else {
            break;
        }
    }
    parse_element(&mut r)
}

fn parse_element(r: &mut Reader) -> Result<Xml, String> {
    if r.bump() != Some(b'<') || r.peek() == Some(b'/') {
        return Err("expected a start tag".into());
    }
    let name = r.until(b" \t\r\n>/").to_string();
    if name.is_empty() {
        return Err("an element has no name".into());
    }
    let mut attrs = Vec::new();
    loop {
        r.skip_ws();
        match r.peek() {
            Some(b'/') => {
                r.bump();
                if r.bump() != Some(b'>') {
                    return Err(format!("<{name}>: bad self-close"));
                }
                return Ok(Xml { name, attrs, children: Vec::new() });
            }
            Some(b'>') => {
                r.bump();
                break;
            }
            Some(_) => {
                let an = r.until(b" \t\r\n=>/").to_string();
                if an.is_empty() {
                    return Err(format!("<{name}>: empty attribute name"));
                }
                r.skip_ws();
                if r.bump() != Some(b'=') {
                    return Err(format!("<{name}> {an}: no '='"));
                }
                r.skip_ws();
                let q = match r.bump() {
                    Some(q @ (b'"' | b'\'')) => q,
                    _ => return Err(format!("<{name}> {an}: value is not quoted")),
                };
                let raw = r.until(&[q]);
                if r.bump() != Some(q) {
                    return Err(format!("<{name}> {an}: unterminated value"));
                }
                attrs.push((an, decode_entities(raw)));
            }
            None => return Err(format!("<{name}>: unterminated start tag")),
        }
    }
    let mut children = Vec::new();
    loop {
        if r.i >= r.b.len() {
            return Err(format!("<{name}> is never closed"));
        }
        if r.at("</") {
            r.i += 2;
            let close = r.until(b">").trim().to_string();
            if r.bump() != Some(b'>') {
                return Err(format!("<{name}>: bad end tag"));
            }
            if close != name {
                return Err(format!("<{name}> closed by </{close}>"));
            }
            return Ok(Xml { name, attrs, children });
        } else if r.at("<!--") {
            r.skip_to("-->");
        } else if r.at("<![CDATA[") {
            r.skip_to("]]>");
        } else if r.at("<?") {
            r.skip_to("?>");
        } else if r.peek() == Some(b'<') {
            children.push(parse_element(r)?);
        } else {
            r.until(b"<");
        }
    }
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        if let Some(j) = after.find(';') {
            let ent = &after[..j];
            let rep = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ => ent.strip_prefix('#').and_then(|n| {
                    if let Some(h) = n.strip_prefix('x').or_else(|| n.strip_prefix('X')) {
                        u32::from_str_radix(h, 16).ok().and_then(char::from_u32)
                    } else {
                        n.parse::<u32>().ok().and_then(char::from_u32)
                    }
                }),
            };
            match rep {
                Some(c) => {
                    out.push(c);
                    rest = &after[j + 1..];
                }
                None => {
                    out.push('&');
                    rest = after;
                }
            }
        } else {
            out.push('&');
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{self, Head, Selector};
    use metal_schema::Signal;

    /// The one test fixture, as a real `.gdtf` body: a 5-channel wash with
    /// 16-bit pan/tilt, a dimmer and two colour bytes. Written out as bytes
    /// (zip) below — a real file, not a mock (rule 11).
    const DESCRIPTION: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<GDTF DataVersion="1.2">
 <FixtureType Name="Test Wash" ShortName="TestWash" Manufacturer="MetalVJ">
  <DMXModes>
   <DMXMode Name="Standard" Geometry="Body">
    <DMXChannels>
     <DMXChannel DMXBreak="1" Offset="1" Geometry="Yoke">
      <LogicalChannel Attribute="Pan" Snap="No">
       <ChannelFunction Attribute="Pan" Name="Pan" DMXFrom="0/2" DMXTo="65535/2" PhysicalFrom="-270" PhysicalTo="270"/>
      </LogicalChannel>
     </DMXChannel>
     <DMXChannel DMXBreak="1" Offset="3" Geometry="Yoke">
      <LogicalChannel Attribute="Tilt" Snap="No">
       <ChannelFunction Attribute="Tilt" Name="Tilt" DMXFrom="0/2" DMXTo="65535/2" PhysicalFrom="-135" PhysicalTo="135"/>
      </LogicalChannel>
     </DMXChannel>
     <DMXChannel DMXBreak="1" Offset="5" Geometry="Body">
      <LogicalChannel Attribute="Dimmer">
       <ChannelFunction Attribute="Dimmer" Name="Dimmer" DMXFrom="0/1" DMXTo="255/1"/>
      </LogicalChannel>
     </DMXChannel>
     <DMXChannel DMXBreak="1" Offset="6" Geometry="Body">
      <LogicalChannel Attribute="ColorAdd_R">
       <ChannelFunction Attribute="ColorAdd_R" Name="Red" DMXFrom="0/1" DMXTo="255/1"/>
      </LogicalChannel>
     </DMXChannel>
     <DMXChannel DMXBreak="1" Offset="7" Geometry="Body">
      <LogicalChannel Attribute="ColorAdd_G">
       <ChannelFunction Attribute="ColorAdd_G" Name="Green" DMXFrom="0/1" DMXTo="255/1"/>
      </LogicalChannel>
     </DMXChannel>
     <DMXChannel DMXBreak="1" Offset="8" Geometry="Body">
      <LogicalChannel Attribute="ColorAdd_B">
       <ChannelFunction Attribute="ColorAdd_B" Name="Blue" DMXFrom="0/1" DMXTo="255/1"/>
      </LogicalChannel>
     </DMXChannel>
    </DMXChannels>
   </DMXMode>
  </DMXModes>
 </FixtureType>
</GDTF>
"#;

    fn signal() -> Signal {
        Signal {
            level: 1.0,
            hue: 0.0,
            sat: 0.0,
            strobe: 0.0,
            pan: 0.0,
            tilt: -1.0,
            a: 1.0,
            b: 0.5,
            cct: 5600.0,
            zoom: 0.0,
            focus: 0.0,
            iris: 0.0,
            frost: 0.0,
        }
    }

    #[test]
    fn a_gdtf_mode_reproduces_its_dmx_bytes() {
        let f = import_bytes(&package(), None, None).expect("import");
        assert_eq!(f.model, "testwash");
        assert_eq!(f.label, "Test Wash");
        // Pan, tilt, dimmer, red (a), green (b); blue has no selector.
        assert_eq!(f.channels.len(), 5);
        assert_eq!(f.limits.pan, [-270.0, 270.0]);
        assert_eq!(f.limits.tilt, [-135.0, 135.0]);

        let pan = f.channels.iter().find(|c| c.signal == Selector::Pan).unwrap();
        assert_eq!((pan.coarse, pan.fine), (Some(0), Some(1)));
        assert_eq!(pan.bytes(&signal()), vec![(0, 0x80), (1, 0x00)]);

        let level = f.channels.iter().find(|c| c.signal == Selector::Level).unwrap();
        assert_eq!(level.bytes(&signal()), vec![(4, 255)]);

        // The whole frame, by absolute address: pan mid, tilt at 0, dimmer and
        // red full, green at 0.5 -> 128.
        let head = Head { fixture: f.model.clone(), universe: None, address: Some(1) };
        let frame = f.frame(&head, &signal());
        assert_eq!(
            frame,
            vec![
                (1, 0x80),
                (2, 0x00),
                (3, 0x00),
                (4, 0x00),
                (5, 255),
                (6, 255),
                (7, 128),
            ]
        );
    }

    #[test]
    fn an_imported_profile_round_trips_through_the_one_parser() {
        let f = import_bytes(&package(), Some("My Fixture"), None).expect("import");
        assert_eq!(f.model, "my-fixture", "the name override is the file stem");
        let back = fixture::parse(&f.model, &f.to_toml().unwrap()).expect("re-reads");
        assert_eq!(back, f);
    }

    #[test]
    fn an_imported_profile_lands_in_the_data_dir_and_loads_back() {
        let root = std::env::temp_dir().join(format!("fw-gdtf-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&root);
        let f = import_bytes(&package(), None, None).expect("import");
        save(&root, &f).expect("write");
        let loaded = fixture::get(&root, "testwash").expect("loads from data/fixtures");
        assert_eq!(loaded, f);
        metal_vfs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_missing_or_bad_file_is_an_error_not_a_panic() {
        assert!(import_bytes(b"not a zip at all", None, None).is_err());
        assert!(from_description_xml("<GDTF></GDTF>", None, None).is_err());
        assert!(from_description_xml("<!-- a comment -->", None, None).is_err());
    }

    /// A real zip around `DESCRIPTION`, deflated (method 8) so the test also
    /// exercises the inflate path a real `.gdtf` takes.
    fn package() -> Vec<u8> {
        zip(&[("description.xml", DESCRIPTION.as_bytes())])
    }

    fn deflate(data: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 {
                let m = crc & 1 != 0;
                crc >>= 1;
                if m {
                    crc ^= 0xEDB8_8320;
                }
            }
        }
        !crc
    }

    fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let entries: Vec<(&str, Vec<u8>, u32, u32)> = files
            .iter()
            .map(|(name, data)| (*name, deflate(data), crc32(data), data.len() as u32))
            .collect();
        let mut out = Vec::new();
        let mut offsets = Vec::new();
        for (name, comp, crc, raw_len) in &entries {
            offsets.push(out.len() as u32);
            out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            out.extend_from_slice(&20u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&8u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(comp.len() as u32).to_le_bytes());
            out.extend_from_slice(&raw_len.to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(comp);
        }
        let cd_start = out.len() as u32;
        for (i, (name, comp, crc, raw_len)) in entries.iter().enumerate() {
            out.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            out.extend_from_slice(&20u16.to_le_bytes());
            out.extend_from_slice(&20u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&8u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(comp.len() as u32).to_le_bytes());
            out.extend_from_slice(&raw_len.to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&offsets[i].to_le_bytes());
            out.extend_from_slice(name.as_bytes());
        }
        let cd_size = out.len() as u32 - cd_start;
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_start.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }
}
