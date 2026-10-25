//! QGIS labels (`<labeling type="simple|rule-based">`) as RustGIS label classes: each rule a
//! class whose SQL clause is its filter (and its parents'), an else rule the features no other
//! rule takes; the text's field, size, colour, weight and slant, its buffer as a halo, its
//! placement and its scale range.

use roxmltree::Node;

use super::{QGIS_SCALE, Report, child, color, data_defined_on, expr, prop, props};
use crate::Rgba;
use crate::symbology::{DEFAULT_LABEL_CLASS, LABEL_SIZE, LabelClass, LabelPlacement, Labeling, MAX_LABEL_CLASSES};

/// A layer's labels from its `<labeling>`, or `None` when none can be carried over.
pub(super) fn read(l: Node, layer: &str, report: &mut Report) -> Option<Labeling> {
    let mut classes: Vec<LabelClass> = Vec::new();
    match l.attribute("type") {
        Some("simple") => {
            let settings = child(l, "settings")?;
            classes.extend(class(settings, DEFAULT_LABEL_CLASS, None, (None, None), layer, report));
        }
        Some("rule-based") => {
            let mut rules = Vec::new();
            if let Some(root) = child(l, "rules") {
                flatten(root, Vec::new(), (None, None), &mut rules, 0);
            }
            for (i, r) in rules.iter().enumerate() {
                let Some(settings) = child(r.node, "settings") else { continue };
                // Switched off: never drawn, so nothing about it is noted.
                if r.node.attribute("active") == Some("0") {
                    continue;
                }
                let name = r
                    .node
                    .attribute("description")
                    .filter(|d| !d.trim().is_empty())
                    .map_or_else(|| format!("Rule {}", i + 1), str::to_string);
                // Its filters and its parents', and for an else rule, none of its siblings'.
                let mut parts: Vec<Option<String>> = r.filters.iter().map(|f| expr::to_sql(f)).collect();
                if let Some(siblings) = &r.else_of {
                    let others: Option<Vec<String>> = siblings.iter().map(|f| expr::to_sql(f)).collect();
                    parts.push(others.map(|o| if o.is_empty() { String::new() } else { format!("NOT (({}))", o.join(") OR (")) }));
                }
                let where_ = parts.into_iter().collect::<Option<Vec<String>>>().map(|p| {
                    let p: Vec<String> = p.into_iter().filter(|c| !c.is_empty()).collect();
                    match p.as_slice() {
                        [] => None,
                        [one] => Some(one.clone()),
                        many => Some(format!("({})", many.join(") AND ("))),
                    }
                });
                let Some(where_) = where_ else {
                    report.skipped(
                        layer,
                        format!(
                            "the label rule \"{name}\", whose filter `{}` doesn't read as a RustGIS SQL clause",
                            r.filters.join(" AND ")
                        ),
                    );
                    continue;
                };
                classes.extend(class(settings, &name, where_, r.scales, layer, report));
            }
        }
        Some(other) => {
            report.skipped(layer, format!("its {other} labels"));
            return None;
        }
        None => return None,
    }
    if classes.is_empty() {
        return None;
    }
    if classes.len() > MAX_LABEL_CLASSES {
        report.skipped(layer, format!("label rules past the first {MAX_LABEL_CLASSES}"));
        classes.truncate(MAX_LABEL_CLASSES);
    }
    // A simple labeling is the Default class; rules are classes of their own, Default off.
    let default = if l.attribute("type") == Some("simple") {
        classes.remove(0)
    } else {
        let first = classes.first()?.clone();
        LabelClass { name: DEFAULT_LABEL_CLASS.into(), visible: false, where_: None, ..first }
    };
    let classes = if l.attribute("type") == Some("simple") { Vec::new() } else { classes };
    Some(Labeling { enabled: true, default, classes })
}

fn is_else(f: &str) -> bool {
    f.trim().eq_ignore_ascii_case("ELSE")
}

/// A rule-based labeling's rule with settings: its filter and its parents' (an else rule's own
/// left out), for an else rule its siblings' filters (it takes what none of them does), and the
/// scale range along the way (RustGIS's min and max scale).
struct Rule<'a, 'i> {
    node: Node<'a, 'i>,
    filters: Vec<String>,
    else_of: Option<Vec<String>>,
    scales: (Option<f64>, Option<f64>),
}

fn flatten<'a, 'i>(
    node: Node<'a, 'i>,
    filters: Vec<String>,
    scales: (Option<f64>, Option<f64>),
    out: &mut Vec<Rule<'a, 'i>>,
    depth: usize,
) {
    if depth > 32 || out.len() > MAX_LABEL_CLASSES * 4 {
        return;
    }
    let rules: Vec<Node> = node.children().filter(|c| c.has_tag_name("rule")).collect();
    // The filters an else rule here is the rest of: its switched-on siblings', as QGIS has it.
    let siblings: Vec<String> = rules
        .iter()
        .filter(|r| r.attribute("active") != Some("0"))
        .filter_map(|r| r.attribute("filter").map(str::trim))
        .filter(|f| !f.is_empty() && !is_else(f))
        .map(str::to_string)
        .collect();
    for rule in rules {
        let mut f = filters.clone();
        let own = rule.attribute("filter").map(str::trim).filter(|f| !f.is_empty());
        let else_of = own.filter(|o| is_else(o)).map(|_| siblings.clone());
        if let Some(filter) = own.filter(|o| !is_else(o)) {
            f.push(filter.to_string());
        }
        // A rule's scalemaxdenom is the out-beyond limit, its scalemindenom the in-beyond.
        let denom = |k: &str| {
            rule.attribute(k).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite() && *v > 0.0).map(|v| v * QGIS_SCALE)
        };
        let s = (denom("scalemaxdenom").or(scales.0), denom("scalemindenom").or(scales.1));
        if child(rule, "settings").is_some() {
            out.push(Rule { node: rule, filters: f.clone(), else_of, scales: s });
        }
        flatten(rule, f, s, out, depth + 1);
    }
}

/// A label class from QGIS's label `settings`, or `None` when its text can't be carried over.
fn class(
    settings: Node,
    name: &str,
    where_: Option<String>,
    scales: (Option<f64>, Option<f64>),
    layer: &str,
    report: &mut Report,
) -> Option<LabelClass> {
    let text = child(settings, "text-style")?;
    let field = text.attribute("fieldName").map(str::trim).unwrap_or_default();
    let field = if text.attribute("isExpression") == Some("1") {
        // An expression that is only a field.
        let bare = field.strip_prefix('"').and_then(|f| f.strip_suffix('"')).unwrap_or(field);
        if bare.is_empty() || !bare.chars().all(|c| c.is_alphanumeric() || c == '_' || c == ' ') {
            report.skipped(layer, format!("labels whose text is the expression `{field}` (only a field's)"));
            return None;
        }
        bare.to_string()
    } else {
        field.to_string()
    };
    if field.is_empty() {
        return None;
    }
    let num = |n: Node, k: &str| n.attribute(k).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite());
    // The size in typographic points, as RustGIS's label sizes are.
    let raw = num(text, "fontSize").unwrap_or(10.0);
    let size = match text.attribute("fontSizeUnit").unwrap_or("Point") {
        "Point" => raw,
        "MM" => raw * 72.0 / 25.4,
        "Pixel" => raw * 0.75,
        "Inch" => raw * 72.0,
        _ => {
            report.approximated(layer, "label sizes in map units, taken at 1:25,000");
            raw / 25_000.0 * 72.0 / 0.0254
        }
    };
    let opacity = num(text, "textOpacity").unwrap_or(1.0).clamp(0.0, 1.0) as f32;
    let c = text.attribute("textColor").and_then(color).unwrap_or(Rgba::rgb(0, 0, 0));
    let c = c.with_alpha((f32::from(c.a()) * opacity).round() as u8);
    let style = text.attribute("namedStyle").unwrap_or_default().to_ascii_lowercase();
    let weight = num(text, "fontWeight").unwrap_or(50.0);
    let bold = style.contains("bold")
        || style.contains("black")
        || style.contains("heavy")
        || weight >= 63.0
        || text.attribute("fontBold") == Some("1");
    let italic = style.contains("italic") || style.contains("oblique") || text.attribute("fontItalic") == Some("1");
    // QGIS's font family: drawn in it where it is installed, else in RustGIS's label font.
    let font = text.attribute("fontFamily").and_then(crate::symbology::font_name);
    if num(text, "fontLetterSpacing").is_some_and(|s| s != 0.0) {
        report.approximated(layer, "label letter spacing, left out");
    }
    let halo = child(text, "text-buffer").is_some_and(|b| b.attribute("bufferDraw") == Some("1"));
    if halo {
        report.approximated(
            layer,
            "label halos in their own colour and width, drawn as RustGIS's (thin, white or dark by the text's colour)",
        );
    }
    if child(text, "background").is_some_and(|b| b.attribute("shapeDraw") == Some("1")) {
        report.approximated(layer, "label backgrounds (shapes behind the text), left out");
    }
    if child(text, "shadow").is_some_and(|b| b.attribute("shadowDraw") == Some("1")) {
        report.approximated(layer, "label shadows, left out");
    }
    let placement = child(settings, "placement").map_or(LabelPlacement::Auto, |p| placement(p, layer, report));
    unread(settings, layer, report);
    // The settings' own scale range, else the rule's.
    let rendering = child(settings, "rendering");
    let (mut min_scale, mut max_scale) = scales;
    if let Some(r) = rendering.filter(|r| r.attribute("scaleVisibility") == Some("1")) {
        min_scale = num(r, "scaleMin").filter(|v| *v > 0.0).map(|v| v * QGIS_SCALE).or(min_scale);
        max_scale = num(r, "scaleMax").filter(|v| *v > 0.0).map(|v| v * QGIS_SCALE).or(max_scale);
    }
    Some(LabelClass {
        name: name.to_string(),
        visible: true,
        where_,
        field,
        size: size.clamp(*LABEL_SIZE.start(), *LABEL_SIZE.end()) as f32,
        color: c,
        halo,
        bold,
        min_scale,
        max_scale,
        closest: None,
        farthest: None,
        placement,
        italic,
        font,
    })
}

/// Notes the label settings RustGIS has no place for, where they are set off QGIS's defaults.
fn unread(settings: Node, layer: &str, report: &mut Report) {
    let num =
        |n: Option<Node>, k: &str| n.and_then(|n| n.attribute(k)).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite());
    let p = child(settings, "placement");
    if num(p, "dist").is_some_and(|d| d != 0.0) {
        report.approximated(layer, "label distances from their features, left out");
    }
    if num(p, "xOffset").is_some_and(|d| d != 0.0) || num(p, "yOffset").is_some_and(|d| d != 0.0) {
        report.approximated(layer, "label offsets, left out");
    }
    if num(p, "repeatDistance").is_some_and(|d| d > 0.0) {
        report.approximated(layer, "labels repeated along lines, drawn once");
    }
    if num(p, "priority").is_some_and(|d| d != 5.0) {
        report.approximated(layer, "label priorities, left out");
    }
    let format = child(settings, "text-format");
    if format.and_then(|f| f.attribute("wrapChar")).is_some_and(|w| !w.is_empty()) || num(format, "autoWrapLength").is_some_and(|w| w > 0.0)
    {
        report.approximated(layer, "wrapped labels, drawn on one line");
    }
    if child(settings, "callout").is_some_and(|c| prop(&props(c), "enabled") == Some("1")) {
        report.skipped(layer, "label callouts (lines from a label to its feature)");
    }
    if data_defined_on(settings) {
        report.skipped(layer, "data-defined label settings (from fields or expressions)");
    }
}

/// QGIS's label placement, as RustGIS's.
fn placement(p: Node, layer: &str, report: &mut Report) -> LabelPlacement {
    let quad = |q: Option<&str>| match q {
        Some("0") => LabelPlacement::AboveLeft,
        Some("1") => LabelPlacement::Above,
        Some("2") => LabelPlacement::AboveRight,
        Some("3") => LabelPlacement::Left,
        Some("5") => LabelPlacement::Right,
        Some("6") => LabelPlacement::BelowLeft,
        Some("7") => LabelPlacement::Below,
        Some("8") => LabelPlacement::BelowRight,
        _ => LabelPlacement::Center,
    };
    match p.attribute("placement").unwrap_or("0") {
        // Around the point: QGIS picks the best spot, RustGIS's automatic is above it.
        "0" | "6" => LabelPlacement::Auto,
        "1" => quad(p.attribute("quadOffset")),
        "2" => LabelPlacement::Parallel,
        "3" => LabelPlacement::Curved,
        "4" | "5" => LabelPlacement::Center,
        other => {
            report.approximated(layer, format!("label placement {}, centred", placement_name(other)));
            LabelPlacement::Center
        }
    }
}

fn placement_name(code: &str) -> &'static str {
    match code {
        "7" => "around polygon outlines",
        "8" => "outside polygons",
        _ => "of an unknown kind",
    }
}
