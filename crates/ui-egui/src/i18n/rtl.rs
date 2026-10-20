//! Display order for lines the UI toolkit lays out back to front.
//!
//! egui has no bidirectional algorithm: it shapes each line of text as one
//! run, in the direction of the line's first strong character. A line that
//! starts with a right-to-left letter is laid out back to front — the shaper
//! reverses its glyphs and mirrors its brackets — which pure right-to-left
//! text survives; but a Latin run inside the line, a product name, a file
//! format, a number, comes out back to front too, so it reads reversed. A
//! line that starts with a left-to-right letter is laid out in logical order,
//! and the right-to-left text in it reads backwards, so the catalogs of such
//! languages start their lines with right-to-left text (see `fa.tsv`).
//!
//! [`visual`] rewrites a back-to-front line into the form that reads correctly
//! once laid out: each left-to-right run is reversed ahead of time, and every
//! bracket is mirrored, for the shaper mirrors every bracket of a back-to-front
//! line, those inside a left-to-right run too. Joining, which the shaper does on
//! the text as given, is untouched: right-to-left text keeps its written
//! order. Each line is rewritten on its own strength, and a line that starts
//! left-to-right, or a string with no right-to-left text at all, comes back
//! unchanged.
//!
//! [`fit`] breaks a long line into lines whose display form fits a width, so
//! the toolkit's own wrapping never has to: it fills rows from the start of a
//! back-to-front line's glyph stream — the end of the sentence — so every row
//! is handed over whole. A row that would start with a left-to-right word
//! while still holding right-to-left text would be laid out front to back and
//! read wrong, so the break moves a word earlier. Laying out the widgets
//! themselves right to left is separate, future work.
//!
//! The run rules are the Unicode bidirectional algorithm's, kept to what one
//! line needs: a neutral joins a run when left-to-right text is on both sides
//! of it (N1), and a bracket when it opens or closes onto a Latin letter (N0).

/// A character of a script written right to left (Arabic, Persian, Hebrew). Digits
/// read left to right, and the comma, semicolon and question mark take the line's
/// direction, so none of them are script letters here.
fn rtl_script(c: char) -> bool {
    if matches!(c as u32, 0x0660..=0x066C | 0x06F0..=0x06F9 | 0x060C | 0x061B | 0x061F) {
        return false;
    }
    matches!(c as u32,
        0x0590..=0x05FF       // Hebrew
        | 0x0600..=0x06FF     // Arabic
        | 0x0750..=0x077F
        | 0x08A0..=0x08FF
        | 0xFB1D..=0xFDFF     // presentation forms
        | 0xFE70..=0xFEFF)
}

/// A character of a left-to-right run: a letter or number of any script but a
/// right-to-left one (Latin, Cyrillic, Greek, CJK, accented letters, superscripts),
/// or a Persian or Arabic digit.
fn ltr(c: char) -> bool {
    c.is_alphanumeric() && !rtl_script(c)
}

/// A digit of either script: what the degree sign attaches to.
fn digit(c: char) -> bool {
    c.is_ascii_digit() || matches!(c as u32, 0x0660..=0x0669 | 0x06F0..=0x06F9)
}

/// A left-to-right letter: what brackets pair onto (they close around letters, not
/// numbers).
fn letter(c: char) -> bool {
    c.is_alphabetic() && !rtl_script(c)
}

/// What the toolkit's direction guess makes of a character: `Some(true)` — of a
/// right-to-left script, the line start that is laid out back to front (the digits
/// of the Arabic block included, for their script is Arabic); `Some(false)` — of a
/// left-to-right script (Latin, Cyrillic, Greek, CJK…); `None` — of no script of
/// its own (digits, punctuation, marks, controls), which the guess skips.
fn strong(c: char) -> Option<bool> {
    if rtl_script(c) || matches!(c as u32, 0x0660..=0x0669 | 0x06F0..=0x06F9) {
        Some(true)
    } else if c.is_alphabetic() {
        Some(false)
    } else {
        None
    }
}

/// The mirrored glyph of a bracket, which the shaper mirrors back on a back-to-front line.
fn mirror(c: char) -> char {
    match c {
        '(' => ')',
        ')' => '(',
        '[' => ']',
        ']' => '[',
        '{' => '}',
        '}' => '{',
        '«' => '»',
        '»' => '«',
        '‹' => '›',
        '›' => '‹',
        c => c,
    }
}

fn opening(c: char) -> bool {
    matches!(c, '(' | '[' | '{' | '«' | '‹')
}

fn closing(c: char) -> bool {
    matches!(c, ')' | ']' | '}' | '»' | '›')
}

/// Whether any right-to-left text is in `s`: a cheap check before any work.
#[must_use]
pub fn has_rtl(s: &str) -> bool {
    s.chars().any(rtl_script)
}

/// Whether `s` starts, past characters of no script of their own, with one of a
/// right-to-left script: the lines the toolkit lays out back to front. A string
/// of no strong character at all is laid out front to back.
fn back_to_front(s: &str) -> bool {
    s.chars().find_map(strong) == Some(true)
}

/// `s` in the form that reads correctly when laid out back to front: each
/// left-to-right run reversed (the line's own reversal puts it back), and every bracket
/// mirrored (the shaper mirrors them all back). Everything else is untouched, so the shaper still
/// joins the right-to-left text as written. Every line is rewritten on its own
/// strength; a line that starts left-to-right, and a string with no right-to-left
/// text, come back unchanged.
#[must_use]
pub fn visual(s: &str) -> String {
    if !has_rtl(s) {
        return s.to_owned();
    }
    s.split('\n').map(visual_line).collect::<Vec<_>>().join("\n")
}

/// One line of [`visual`]: rewritten only when it starts back to front.
fn visual_line(line: &str) -> String {
    if !back_to_front(line) {
        return line.to_owned();
    }
    let chars: Vec<char> = line.chars().collect();
    // The nearest run character before and after each position (1 left-to-right,
    // 2 right-to-left): a neutral between two runs takes the side of both.
    let mut before = vec![0u8; chars.len()];
    let mut seen = 0u8;
    for (i, c) in chars.iter().enumerate() {
        before[i] = seen;
        if ltr(*c) {
            seen = 1;
        } else if rtl_script(*c) {
            seen = 2;
        }
    }
    let mut after = vec![0u8; chars.len()];
    let mut next = 0u8;
    for i in (0..chars.len()).rev() {
        after[i] = next;
        if ltr(chars[i]) {
            next = 1;
        } else if rtl_script(chars[i]) {
            next = 2;
        }
    }
    // The nearest Latin letter before and after: a bracket joins a run when it opens
    // or closes onto a letter, not onto a number (Unicode 'paired brackets').
    let mut letter_before = vec![0u8; chars.len()];
    let mut word_seen = 0u8;
    for (i, c) in chars.iter().enumerate() {
        letter_before[i] = word_seen;
        if letter(*c) {
            word_seen = 1;
        } else if rtl_script(*c) {
            word_seen = 2;
        }
    }
    let mut letter_after = vec![0u8; chars.len()];
    let mut word_next = 0u8;
    for i in (0..chars.len()).rev() {
        letter_after[i] = word_next;
        if letter(chars[i]) {
            word_next = 1;
        } else if rtl_script(chars[i]) {
            word_next = 2;
        }
    }
    // Paired brackets are resolved as one (Unicode BD16, N0): a pair joins the run
    // only when both of its brackets would, and nothing right-to-left is inside, so
    // its two brackets never land in different runs.
    let mut partner: Vec<Option<usize>> = vec![None; chars.len()];
    let mut open: Vec<usize> = Vec::new();
    for (i, c) in chars.iter().enumerate() {
        if opening(*c) {
            open.push(i);
        } else if closing(*c)
            && let Some(at) = open.iter().rposition(|o| chars.get(*o).copied().map(mirror) == Some(*c))
        {
            let o = open[at];
            open.truncate(at);
            partner[o] = Some(i);
            partner[i] = Some(o);
        }
    }
    let alone = |i: usize| {
        if opening(chars[i]) { letter_after[i] == 1 } else { letter_before[i] == 1 }
    };
    let in_run = |i: usize| {
        let c = chars[i];
        if ltr(c) {
            return true;
        }
        if rtl_script(c) {
            return false;
        }
        if c == '°' {
            // A European terminator: of the run when a digit is next to it, of
            // the line's flow otherwise.
            return (0 < i && digit(chars[i - 1])) || (i + 1 < chars.len() && digit(chars[i + 1]));
        }
        if opening(c) || closing(c) {
            match partner[i] {
                Some(j) => {
                    let (o, e) = (i.min(j), i.max(j));
                    alone(o) && alone(e) && !chars[o..e].iter().copied().any(rtl_script)
                }
                None => alone(i),
            }
        } else {
            before[i] == 1 && after[i] == 1
        }
    };
    let mut out = String::with_capacity(line.len());
    let mut run: Vec<char> = Vec::new();
    for (i, c) in chars.iter().enumerate() {
        if in_run(i) {
            run.push(*c);
        } else {
            out.extend(run.drain(..).rev().map(mirror));
            out.push(mirror(*c));
        }
    }
    out.extend(run.drain(..).rev().map(mirror));
    out
}

/// `s` broken into lines whose display form is no wider than `max_width`, so the
/// toolkit's own wrapping never has to: it fills rows from the start of a
/// back-to-front line's glyph stream — the end of the sentence — so every row is
/// handed over whole, in display form. A row that starts with a left-to-right word
/// is laid out front to back, and the right-to-left words in it would read
/// backwards, so it ends before the first word with right-to-left text; a row with
/// no right-to-left text at all reads correctly front to back on its own. `width`
/// measures a line's display form. Strings with no right-to-left text come back
/// unchanged.
#[must_use]
pub fn fit(s: &str, max_width: f32, width: &dyn Fn(&str) -> f32) -> String {
    if !has_rtl(s) || !max_width.is_finite() || max_width <= 0.0 {
        return s.to_owned();
    }
    let mut rows = Vec::new();
    for line in s.split('\n') {
        if back_to_front(line) {
            fit_line(line, max_width, width, &mut rows);
        } else {
            rows.push(line.to_owned());
        }
    }
    rows.join("\n")
}

/// One line of [`fit`]: rows of whole words that fit, each in display form.
fn fit_line(line: &str, max_width: f32, width: &dyn Fn(&str) -> f32, rows: &mut Vec<String>) {
    let words: Vec<&str> = line.split(' ').collect();
    let mut start = 0;
    while start < words.len() {
        // The widest run of words that fits, always at least one word: a single
        // word wider than the row goes on one of its own.
        let mut end = start + 1;
        while end < words.len() && width(&visual_line(&words[start..=end].join(" "))) <= max_width {
            end += 1;
        }
        // A row whose first strong word is left-to-right is laid out front to
        // back, and the right-to-left words in it would read backwards: the row
        // ends before the first word that holds right-to-left text.
        if words[start..end].iter().find_map(|w| w.chars().find_map(strong)) == Some(false)
            && let Some(cut) = (start + 1..end).find(|&i| has_rtl(words[i]))
        {
            end = cut;
        }
        rows.push(visual_line(&words[start..end].join(" ")));
        start = end;
    }
}

#[cfg(test)]
mod tests {
    use super::{fit, visual};
    use crate::i18n::{Lang, fmt, set_current, tr};

    #[test]
    fn text_without_latin_runs_is_untouched() {
        assert_eq!(visual("سلام دوست من"), "سلام دوست من");
        assert_eq!(visual("Symbology"), "Symbology");
        assert_eq!(visual(""), "");
        // A line that starts left-to-right — also past a digit or a mark, which
        // count as none — is laid out in logical order, as is a string with no
        // right-to-left text at all.
        assert_eq!(visual("Symbology پنل"), "Symbology پنل");
        assert_eq!(visual("«{b}» نام"), "«{b}» نام");
    }

    #[test]
    fn the_first_strong_character_decides_the_line() {
        // Digits and marks are of no script of their own: the Persian text after
        // them still lays the line out back to front, so the Latin run inside is
        // rewritten as on any other back-to-front line.
        assert_eq!(visual("۳ باکت S3"), "۳ باکت 3S");
        assert_eq!(visual("3 باکت S3"), "3 باکت 3S");
        assert_eq!(visual("(کشورها، ایالت‌ها) در Natural Earth"), ")کشورها، ایالت‌ها( در htraE larutaN");
    }

    #[test]
    fn latin_runs_reverse_for_display() {
        // The separator space stays in the line's flow; a space inside a Latin phrase
        // joins the run, so the phrase doesn't break apart.
        assert_eq!(visual("پنل Symbology"), "پنل ygolobmyS");
        assert_eq!(visual("باکت‌های S3"), "باکت‌های 3S");
        assert_eq!(visual("نسخه 2.0 است"), "نسخه 0.2 است");
        assert_eq!(visual("دو S3 Bucket است"), "دو tekcuB 3S است");
        // Letters of other left-to-right scripts, and accented ones, are runs too.
        assert_eq!(visual("افزوده شد: Москва"), "افزوده شد: авксоМ");
        assert_eq!(visual("افزوده شد: Café"), "افزوده شد: éfaC");
    }

    #[test]
    fn brackets_pair_onto_letters_and_mirror_in_flow() {
        // Brackets that close onto Latin letters join the run as a unit, and mirror
        // as well: the shaper mirrors every bracket of the line, the run's too.
        assert_eq!(visual("مجوز (CC BY 4.0) است"), "مجوز (0.4 YB CC) است");
        assert_eq!(visual("قالب {z}/{x}/{y} است"), "قالب {y}/{x}/{z} است");
        assert_eq!(visual("مرکاتور وب (WGS 84)"), "مرکاتور وب (48 SGW)");
        // A pair is resolved as one: with right-to-left text inside, both brackets
        // stay in the line's flow, and the Latin words inside are runs of their own.
        assert_eq!(visual("پنل (Symbology و GPU) است"), "پنل )ygolobmyS و UPG( است");
        // …and those around right-to-left text mirror in its flow.
        assert_eq!(visual("پنل (نماد) است"), "پنل )نماد( است");
    }

    #[test]
    fn lines_are_rewritten_each_on_their_own_strength() {
        assert_eq!(visual("پنل Symbology\nSymbology پنل"), "پنل ygolobmyS\nSymbology پنل");
        assert_eq!(visual("GPU\nرندر"), "GPU\nرندر");
    }

    #[test]
    fn long_lines_break_where_the_next_row_stays_back_to_front() {
        let chars = |s: &str| s.chars().count() as f32;
        // Every row of one word.
        assert_eq!(fit("الف بت ج", 2.0, &chars), "الف\nبت\nج");
        // A line that fits stays whole, in display form.
        assert_eq!(fit("پنل Symbology", 30.0, &chars), "پنل ygolobmyS");
        // A row that starts with a Latin word holds no right-to-left words, so
        // "Symbology" stands on a row of its own before "است"; the row before it
        // takes the room it fits.
        assert_eq!(fit("پنل خوب Symbology است", 8.0, &chars), "پنل خوب\nSymbology\nاست");
        // Text with no right-to-left letters is left to the toolkit.
        assert_eq!(fit("hello world", 2.0, &chars), "hello world");
    }

    #[test]
    fn a_row_that_starts_with_a_latin_word_holds_no_rtl_words() {
        let chars = |s: &str| s.chars().count() as f32;
        // The row "Symbology است" would be laid out front to back and show "است"
        // backwards, so the Latin word stands on a row of its own.
        assert_eq!(fit("خوبتر Symbology است", 14.0, &chars), "خوبتر\nSymbology\nاست");
    }

    #[test]
    fn the_catalog_and_messages_show_in_display_form() {
        let fa = Lang::from_code("fa").unwrap();
        assert_eq!(tr(fa, "List S3 Bucket"), "فهرست باکت 3S");
        assert_eq!(tr(fa, "3D mesh"), "مش سه‌بعدی");
        // Placeholders are filled in logical order, then the message is displayed.
        // `fmt` reads the thread's current language, and a test thread starts in
        // English: pin it to Persian, and put it back for whoever runs next.
        set_current(fa);
        assert_eq!(fmt("Added: {names}", &[("names", "Cities")]), "افزوده شد: seitiC");
        set_current(Lang::EN);
    }
}
