//! Display units for measurements and distance controls. Inputs stay in metres or square metres.
use rustgis_engine::geom;

/// Keep nonzero measurements visible when rounding to whole units would erase them.
fn whole_or_small(value: f64) -> String {
    let magnitude = value.abs();
    if magnitude > 0.0 && magnitude <= 0.5 {
        if magnitude < 0.0001 {
            format!("{value:.1e}")
        } else {
            // Two significant figures, with at most five decimal places in this range.
            let precision = (1.0 - magnitude.log10().floor()) as usize;
            format!("{value:.precision$}")
        }
    } else {
        format!("{value:.0}")
    }
}

/// Format a distance supplied in metres, using feet/miles or the metric formatter.
/// Positive sub-foot values that would round to zero retain significant digits.
pub fn distance(m: f64, imperial: bool) -> String {
    if !imperial {
        return geom::format_distance(m);
    }
    let ft = m / geom::METRES_PER_FOOT;
    if ft.round() < 5280.0 {
        crate::i18n::fmt("{distance} ft", &[("distance", &whole_or_small(ft))])
    } else {
        crate::i18n::fmt("{distance} mi", &[("distance", &format!("{:.1}", m / geom::METRES_PER_MILE))])
    }
}

/// Format an elevation supplied in metres; imperial vertical measurements always use feet.
pub fn elevation(m: f64, imperial: bool) -> String {
    if imperial {
        crate::i18n::fmt("{distance} ft", &[("distance", &whole_or_small(m / geom::METRES_PER_FOOT))])
    } else {
        crate::i18n::fmt("{distance} m", &[("distance", &rustgis_engine::map::symbology::fmt_num(m))])
    }
}

/// Format square metres as square feet, acres or square miles, or use metric units.
/// Positive sub-square-foot values that would round to zero retain significant digits.
pub fn area(m2: f64, imperial: bool) -> String {
    if !imperial {
        return geom::format_area(m2);
    }
    if m2 < geom::SQUARE_METRES_PER_ACRE {
        crate::i18n::fmt("{area} ft²", &[("area", &whole_or_small(m2 / geom::SQUARE_METRES_PER_SQUARE_FOOT))])
    } else if m2 < geom::SQUARE_METRES_PER_SQUARE_MILE {
        crate::i18n::fmt("{area} acres", &[("area", &format!("{:.1}", m2 / geom::SQUARE_METRES_PER_ACRE))])
    } else {
        crate::i18n::fmt("{area} mi²", &[("area", &format!("{:.1}", m2 / geom::SQUARE_METRES_PER_SQUARE_MILE))])
    }
}

/// Choose a readable scale-bar length in the display units and return its length in metres.
pub fn scale_distance(max_m: f64, imperial: bool) -> f64 {
    let unit = if !imperial {
        1.0
    } else if max_m < geom::METRES_PER_MILE {
        geom::METRES_PER_FOOT
    } else {
        geom::METRES_PER_MILE
    };
    geom::nice_distance(max_m / unit) * unit
}
