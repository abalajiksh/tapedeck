//! The Indian scripts, in two styles.
//!
//! **ISO 15919** is strict: every letter mapped, every inherent vowel written,
//! nothing guessed. **Readable** is how lyric sites spell a song — no
//! diacritics, `aa`/`ee`/`oo` for long vowels — and that needs two things a
//! table cannot do, both ported from Aksharamukha (virtualvinodh/aksharamukha-
//! python, AGPL-3.0):
//!
//! * **Tamil voicing** (its `TamilTranscribe`). Tamil script does not mark
//!   k/g, c/s/j, ṭ/ḍ, t/d or p/b; which one is meant follows from where the
//!   letter sits — voiced between vowels and after a nasal or a liquid,
//!   voiceless at the start of a word and doubled. So மதம் is "madham", not
//!   "matam".
//! * **Schwa deletion** (its `RemoveSchwaHindi`). Devanagari writes an `a`
//!   after every consonant that Hindi often does not say: तुमने is "tumne",
//!   not "tumane". Bengali, Gujarati and Punjabi drop only the word-final one.
//!
//! On top of Aksharamukha's readable output, Hindi follows lyric-site
//! spelling, as Ashwin chose it on 2026-10-14: a long vowel at the end of a
//! word is written short (क्या "kya", मेरी "meri"), a nasal at the end of a
//! word is `n` (में "mein", नहीं "nahin", यूँ "yun"), and its apostrophe
//! markers are dropped throughout.
//!
//! The letter tables are generated from Aksharamukha's `script_mapping.json`
//! into `indic_tables.rs`.

use super::indic_tables as t;
use super::{IndicStyle, Part, Word};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Vowel,
    Sign,
    Consonant,
    Virama,
    Chandrabindu,
    Anusvara,
    Visarga,
    Nukta,
    Aytham,
    Om,
    Danda,
    Digit,
}

#[derive(Debug)]
pub(super) struct Sym {
    pub c: char,
    pub kind: Kind,
    pub iso: &'static str,
    pub readable: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Script {
    Devanagari,
    Bengali,
    Gurmukhi,
    Gujarati,
    Tamil,
    Telugu,
    Kannada,
    Malayalam,
}

/// How much of the inherent vowel readable spelling drops.
#[derive(PartialEq, Eq)]
enum Schwa {
    /// Hindi: word-final, and medially in V C a C V.
    Full,
    /// Only at the end of a word.
    Final,
    /// Dravidian languages say it.
    Never,
}

impl Script {
    pub(super) fn of(c: char) -> Option<Self> {
        Some(match c as u32 {
            0x0900..=0x097F => Self::Devanagari,
            0x0980..=0x09FF => Self::Bengali,
            0x0A00..=0x0A7F => Self::Gurmukhi,
            0x0A80..=0x0AFF => Self::Gujarati,
            0x0B80..=0x0BFF => Self::Tamil,
            0x0C00..=0x0C7F => Self::Telugu,
            0x0C80..=0x0CFF => Self::Kannada,
            0x0D00..=0x0D7F => Self::Malayalam,
            _ => return None,
        })
    }

    fn table(self) -> &'static [Sym] {
        match self {
            Self::Devanagari => t::DEVANAGARI,
            Self::Bengali => t::BENGALI,
            Self::Gurmukhi => t::GURMUKHI,
            Self::Gujarati => t::GUJARATI,
            Self::Tamil => t::TAMIL,
            Self::Telugu => t::TELUGU,
            Self::Kannada => t::KANNADA,
            Self::Malayalam => t::MALAYALAM,
        }
    }

    fn nukta(self) -> &'static [(char, &'static str, &'static str)] {
        match self {
            Self::Devanagari => t::DEVANAGARI_NUKTA,
            Self::Bengali => t::BENGALI_NUKTA,
            Self::Gurmukhi => t::GURMUKHI_NUKTA,
            Self::Gujarati => t::GUJARATI_NUKTA,
            Self::Tamil => t::TAMIL_NUKTA,
            Self::Telugu => t::TELUGU_NUKTA,
            Self::Kannada => t::KANNADA_NUKTA,
            Self::Malayalam => t::MALAYALAM_NUKTA,
        }
    }

    fn schwa(self) -> Schwa {
        match self {
            Self::Devanagari => Schwa::Full,
            Self::Bengali | Self::Gurmukhi | Self::Gujarati => Schwa::Final,
            _ => Schwa::Never,
        }
    }

    /// Lyric-site spelling of long vowels and final nasals (see the module
    /// note) is the north Indian convention; Dravidian lyrics keep `aa`/`ee`.
    fn north(self) -> bool {
        matches!(self, Self::Devanagari | Self::Bengali | Self::Gurmukhi | Self::Gujarati)
    }

    fn sym(self, c: char) -> Option<&'static Sym> {
        self.table().iter().find(|s| s.c == c)
    }
}

/// Characters the positional table has no single-character entry for.
fn special(c: char) -> Option<Special> {
    Some(match c {
        // Malayalam chillus: a consonant with no vowel, written as one letter.
        'ൺ' => Special::Chillu('ണ'),
        'ൻ' => Special::Chillu('ന'),
        'ർ' => Special::Chillu('ര'),
        'ൽ' => Special::Chillu('ല'),
        'ൾ' => Special::Chillu('ള'),
        'ൿ' => Special::Chillu('ക'),
        // Bengali khanda ta.
        'ৎ' => Special::Chillu('ত'),
        // Gurmukhi tippi (a nasal), addak (doubles what follows) and the two
        // vowel carriers.
        'ੰ' => Special::Nasal,
        'ੱ' => Special::Addak,
        'ੲ' | 'ੳ' => Special::Carrier,
        // Nukta signs: the positional table carries none it can use (its
        // readable column holds a placeholder there).
        '\u{093C}' | '\u{09BC}' | '\u{0A3C}' | '\u{0ABC}' | '\u{0C3C}' | '\u{0CBC}' => {
            Special::Nukta
        }
        '\u{200C}' | '\u{200D}' => Special::Skip,
        'ऽ' => Special::Skip,
        _ => return None,
    })
}

enum Special {
    Chillu(char),
    Nasal,
    Addak,
    Carrier,
    Nukta,
    Skip,
}

/// One written syllable: a consonant (or none), the vowel it carries, and what
/// is stacked on it.
#[derive(Default, Clone)]
struct Ak {
    cons: Option<&'static Sym>,
    /// `(iso, readable)` when a nukta changed the consonant.
    nukta: Option<(&'static str, &'static str)>,
    /// A vowel sign, or the independent vowel when there is no consonant.
    vowel: Option<&'static Sym>,
    /// Followed by a virama: no vowel at all.
    killed: bool,
    nasal: Option<Kind>,
    visarga: bool,
    /// Gurmukhi addak: this consonant is doubled.
    doubled: bool,
    /// Tamil ஃ.
    aytham: bool,
    /// Anything carried as written (digits, dandas, om).
    lit: Option<(&'static str, &'static str)>,
    /// A Gurmukhi vowel carrier with no sign on it yet.
    carrier: bool,
}

impl Ak {
    fn inherent(&self) -> bool {
        self.cons.is_some() && self.vowel.is_none() && !self.killed
    }
}

fn parse(run: &str, script: Script) -> Vec<Ak> {
    let mut aks: Vec<Ak> = Vec::new();
    let mut addak = false;
    // Nukta letters (क़ ड़ ਖ਼ য়) are excluded from Unicode composition, so text
    // may hold either form; split the precomposed one into base + nukta so
    // both read the same.
    let mut chars: Vec<char> = Vec::with_capacity(run.len());
    for c in run.chars() {
        let mut parts = Vec::with_capacity(2);
        unicode_normalization::char::decompose_canonical(c, |d| parts.push(d));
        let nukta = parts.len() == 2 && matches!(special(parts[1]), Some(Special::Nukta));
        if nukta {
            chars.extend(parts);
        } else {
            chars.push(c);
        }
    }
    for c in chars {
        if let Some(sp) = special(c) {
            match sp {
                Special::Chillu(base) => {
                    if let Some(s) = script.sym(base) {
                        aks.push(Ak { cons: Some(s), killed: true, ..Ak::default() });
                    }
                }
                Special::Nasal => {
                    if let Some(a) = aks.last_mut() {
                        a.nasal = Some(Kind::Anusvara);
                    }
                }
                Special::Addak => addak = true,
                Special::Carrier => aks.push(Ak { carrier: true, ..Ak::default() }),
                Special::Nukta => apply_nukta(aks.last_mut(), script),
                Special::Skip => {}
            }
            continue;
        }
        let Some(sym) = script.sym(c) else {
            continue;
        };
        match sym.kind {
            Kind::Consonant => {
                aks.push(Ak { cons: Some(sym), doubled: addak, ..Ak::default() });
                addak = false;
            }
            Kind::Vowel => aks.push(Ak { vowel: Some(sym), ..Ak::default() }),
            Kind::Sign => match aks.last_mut() {
                Some(a) if (a.cons.is_some() || a.carrier) && a.vowel.is_none() && !a.killed => {
                    a.vowel = Some(sym);
                    a.carrier = false;
                }
                // A sign with nothing to sit on: written as its vowel.
                _ => aks.push(Ak { vowel: Some(sym), ..Ak::default() }),
            },
            Kind::Virama => {
                if let Some(a) = aks.last_mut() {
                    a.killed = true;
                }
            }
            Kind::Nukta => apply_nukta(aks.last_mut(), script),
            Kind::Chandrabindu | Kind::Anusvara => match aks.last_mut() {
                Some(a) => a.nasal = Some(sym.kind),
                None => aks.push(Ak { lit: Some((sym.iso, "n")), ..Ak::default() }),
            },
            Kind::Visarga => {
                if let Some(a) = aks.last_mut() {
                    a.visarga = true;
                }
            }
            Kind::Aytham => aks.push(Ak { aytham: true, ..Ak::default() }),
            Kind::Om | Kind::Danda | Kind::Digit => {
                aks.push(Ak { lit: Some((sym.iso, sym.readable)), ..Ak::default() })
            }
        }
    }
    aks
}

fn apply_nukta(last: Option<&mut Ak>, script: Script) {
    let Some(a) = last else { return };
    let Some(base) = a.cons else { return };
    if let Some((_, iso, rd)) = script.nukta().iter().find(|(b, _, _)| *b == base.c) {
        a.nukta = Some((iso, rd));
    }
}

/// Aksharamukha's readable marks length and retroflexion with apostrophes
/// (`t'`, `e'`); lyric spelling does not.
fn plain(s: &str) -> String {
    s.chars().filter(|c| *c != '\'' && *c != '×').collect()
}

pub(super) fn word(run: &str, script: Script, style: IndicStyle) -> Word {
    let aks = parse(run, script);
    let r = match style {
        IndicStyle::Iso => iso(&aks),
        IndicStyle::Readable if script == Script::Tamil => tamil_readable(&aks),
        IndicStyle::Readable => readable(&aks, script),
    };
    Word { parts: vec![Part { o: run.to_string(), r, lit: false }], syl: false }
}

fn iso(aks: &[Ak]) -> String {
    let mut out = String::new();
    for a in aks {
        if let Some((iso, _)) = a.lit {
            out.push_str(iso);
            continue;
        }
        if a.aytham {
            out.push('ḵ');
            continue;
        }
        if let Some(c) = a.cons {
            let letters = a.nukta.map(|(i, _)| i).unwrap_or(c.iso);
            if a.doubled {
                out.push_str(letters);
            }
            out.push_str(letters);
            if a.inherent() {
                out.push('a');
            }
        }
        if let Some(v) = a.vowel {
            out.push_str(v.iso);
        }
        match a.nasal {
            Some(Kind::Chandrabindu) => out.push_str("m̐"),
            Some(_) => out.push('ṁ'),
            None => {}
        }
        if a.visarga {
            out.push('ḥ');
        }
    }
    out
}

/// Which inherent vowels readable spelling drops.
fn schwa(aks: &[Ak], script: Script) -> Vec<bool> {
    let mut drop = vec![false; aks.len()];
    let rule = script.schwa();
    if rule == Schwa::Never || aks.len() < 2 {
        return drop;
    }
    let last = aks.len() - 1;
    if aks[last].inherent() && aks[last].nasal.is_none() {
        drop[last] = true;
    }
    if rule == Schwa::Full {
        let sounded = |a: &Ak, dropped: bool| {
            (a.cons.is_some() && !a.killed && !dropped) || (a.cons.is_none() && a.vowel.is_some())
        };
        // Right to left, so a deletion to the right is seen: समझना is
        // sa-majh-naa, and its म keeps its vowel because झ lost its own.
        for i in (1..last).rev() {
            let a = &aks[i];
            if !a.inherent() || a.nasal.is_some() {
                continue;
            }
            let prev = &aks[i - 1];
            let next = &aks[i + 1];
            if sounded(prev, drop[i - 1])
                && next.cons.is_some()
                && sounded(next, drop[i + 1])
                && !drop[i - 1]
            {
                drop[i] = true;
            }
        }
    }
    drop
}

fn labial(a: Option<&Ak>) -> bool {
    a.and_then(|a| a.cons).is_some_and(|c| {
        let r = c.readable;
        r.starts_with('p') || r.starts_with('b') || r.starts_with('m')
    })
}

/// A vowel's readable form in north Indian lyric spelling. `end` is whether it
/// ends the word (nothing written after it).
fn north_vowel(iso: &str, end: bool) -> &'static str {
    match (iso, end) {
        ("a", _) => "a",
        ("ā", true) => "a",
        ("ā", false) => "aa",
        ("i", _) => "i",
        ("ī", true) => "i",
        ("ī", false) => "ee",
        ("u", _) => "u",
        ("ū", true) => "u",
        ("ū", false) => "oo",
        ("ē" | "e" | "æ", _) => "e",
        ("ai", _) => "ai",
        ("ō" | "o" | "ô", _) => "o",
        ("au", _) => "au",
        ("r̥", _) => "ri",
        ("r̥̄", _) => "ree",
        _ => "",
    }
}

/// A vowel nasalized at the end of a word, as lyric sites write it: में
/// "mein", नहीं "nahin", यूँ "yun", क्यों "kyon", यहाँ "yahaan".
fn north_final_nasal(iso: &str) -> &'static str {
    match iso {
        "a" => "an",
        "ā" => "aan",
        "i" | "ī" => "in",
        "u" | "ū" => "un",
        "ē" | "e" | "æ" => "ein",
        "ai" => "ain",
        "ō" | "o" | "ô" => "on",
        "au" => "aun",
        _ => "n",
    }
}

fn readable(aks: &[Ak], script: Script) -> String {
    let drop = schwa(aks, script);
    let north = script.north();
    let mut out = String::new();
    for (i, a) in aks.iter().enumerate() {
        let next = aks.get(i + 1);
        let end = next.is_none();
        if let Some((_, rd)) = a.lit {
            out.push_str(rd);
            continue;
        }
        if let Some(c) = a.cons {
            // ज्ञ is "gy" in Hindi (ज्ञान "gyaan"), not the letters' "jnj".
            let gy = north
                && c.c == 'ज'
                && a.killed
                && next.and_then(|n| n.cons).is_some_and(|n| n.c == 'ञ');
            if gy {
                out.push('g');
                continue;
            }
            let after_gy = north
                && c.c == 'ञ'
                && i > 0
                && aks[i - 1].cons.is_some_and(|p| p.c == 'ज')
                && aks[i - 1].killed;
            let letters = if after_gy {
                "y".to_string()
            } else {
                plain(a.nukta.map(|(_, r)| r).unwrap_or(c.readable))
            };
            if a.doubled {
                if let Some(f) = letters.chars().next() {
                    out.push(f);
                }
            }
            out.push_str(&letters);
        }
        // The vowel this syllable says, as its ISO name.
        let vowel_iso: Option<&str> = match (a.cons, a.vowel) {
            (_, Some(v)) => Some(v.iso),
            (Some(_), None) if a.inherent() && !drop[i] => Some("a"),
            _ => None,
        };
        let nasal_end = a.nasal.is_some() && end;
        // ए straight after a long ā is "ye" in lyric spelling, as if written
        // ये: आए "aaye", मुस्कुराए "muskuraaye".
        let glide = north
            && a.cons.is_none()
            && vowel_iso == Some("ē")
            && i > 0
            && matches!(
                (aks[i - 1].cons, aks[i - 1].vowel.map(|v| v.iso)),
                (_, Some("ā")) | (None, Some("ā"))
            );
        if glide {
            out.push('y');
        }
        match vowel_iso {
            Some(v) if north && nasal_end => out.push_str(north_final_nasal(v)),
            Some(v) if north => out.push_str(north_vowel(v, end && a.nasal.is_none())),
            Some(_) => {
                let v = a.vowel.map(|v| plain(v.readable)).unwrap_or_else(|| "a".to_string());
                out.push_str(&v);
            }
            None => {}
        }
        if let Some(_n) = a.nasal {
            if !(north && nasal_end && vowel_iso.is_some()) {
                // Before a consonant the nasal takes its place: रंग "rang",
                // अंबर "ambar". At the end of a Dravidian word it is "m".
                out.push(if end && !north || labial(next) { 'm' } else { 'n' });
            }
        }
        if a.visarga {
            out.push('h');
        }
    }
    out
}

// ── Tamil ───────────────────────────────────────────────────────────────────

const KA: char = 'க';
const CA: char = 'ச';
const TTA: char = 'ட';
const TA: char = 'த';
const PA: char = 'ப';
const RRA: char = 'ற';
const NNNA: char = 'ன';

/// The nasal that voices a stop after it: ங்க, ஞ்ச, ண்ட, ந்த, ம்ப.
fn homorganic(stop: char) -> char {
    match stop {
        KA => 'ங',
        CA => 'ஞ',
        TTA => 'ண',
        TA => 'ந',
        _ => 'ம',
    }
}

/// Letters after which a stop is voiced, from Aksharamukha's `ConMedials`.
fn medial(c: char) -> bool {
    matches!(c, 'ய' | 'ர' | 'ல' | 'ள' | 'ழ' | 'ன')
}

/// A Tamil stop as said where it stands.
fn tamil_stop(aks: &[Ak], i: usize) -> &'static str {
    let a = &aks[i];
    let c = a.cons.map(|c| c.c).unwrap_or(KA);
    let prev = i.checked_sub(1).map(|j| &aks[j]);
    let prev_c = prev.and_then(|p| p.cons).map(|p| p.c);
    let prev_killed = prev.is_some_and(|p| p.killed);
    let geminate = prev_killed && prev_c == Some(c);

    // Voiceless: at the start, doubled, closing a cluster, or after another
    // stop. Voiced: between vowels, after its own nasal or a liquid.
    let voiced = match prev {
        None => false,
        Some(_) if a.killed || geminate => false,
        Some(p) if p.aytham => true,
        Some(_) if prev_killed => {
            let pc = prev_c.unwrap_or(' ');
            pc == homorganic(c) || medial(pc)
        }
        Some(_) => true,
    };
    match (c, voiced) {
        (KA, false) => "k",
        (KA, true) => "g",
        // ச is "s" at the start and between vowels, "j" after ஞ், and "ch"
        // doubled or after a stop.
        (CA, true) if prev_c == Some('ஞ') && prev_killed => "j",
        (CA, true) => "s",
        (CA, false) if prev.is_none() => "s",
        (CA, false) => "ch",
        (TTA, false) => "t",
        (TTA, true) => "d",
        (TA, false) => "th",
        (TA, true) => "dh",
        (_, false) => "p",
        (_, true) => "b",
    }
}

fn tamil_readable(aks: &[Ak]) -> String {
    let mut out = String::new();
    for (i, a) in aks.iter().enumerate() {
        if let Some((_, rd)) = a.lit {
            out.push_str(rd);
            continue;
        }
        if a.aytham {
            out.push('h');
            continue;
        }
        if let Some(c) = a.cons {
            let prev = i.checked_sub(1).map(|j| &aks[j]);
            let next = aks.get(i + 1);
            let letters: String = match c.c {
                KA | CA | TTA | TA | PA => tamil_stop(aks, i).to_string(),
                // ற்ற is "tr" (காற்று "kaatru"), ன்ற is "ndr" (என்று "endru").
                RRA if a.killed && next.and_then(|n| n.cons).is_some_and(|n| n.c == RRA) => {
                    "t".into()
                }
                RRA if prev.is_some_and(|p| p.killed && p.cons.is_some_and(|c| c.c == NNNA)) => {
                    "dr".into()
                }
                // ங் and ஞ் before their own stop are the "n" of "ng", "nj":
                // தங்கம் "thangam", நெஞ்சம் "nenjam" — not "ngg", "njj".
                'ங' | 'ஞ'
                    if a.killed
                        && next.and_then(|n| n.cons).is_some_and(|n| homorganic(n.c) == c.c) =>
                {
                    "n".into()
                }
                _ => plain(c.readable),
            };
            out.push_str(&letters);
            if a.inherent() {
                out.push('a');
            }
        }
        if let Some(v) = a.vowel {
            out.push_str(&plain(v.readable));
        }
        if a.nasal.is_some() {
            out.push('m');
        }
        if a.visarga {
            out.push('h');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(s: &str, style: IndicStyle) -> String {
        s.split(' ')
            .map(|w| {
                let script = w.chars().find_map(Script::of);
                match script {
                    Some(sc) => {
                        let (core, tail) = w.split_at(
                            w.char_indices()
                                .rev()
                                .find(|(_, c)| Script::of(*c).is_some())
                                .map(|(i, c)| i + c.len_utf8())
                                .unwrap_or(w.len()),
                        );
                        format!("{}{}", word(core, sc, style).parts[0].r, tail)
                    }
                    None => w.to_string(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Short excerpts and constructed phrases only: lyrics are third-party
    /// text and stay out of the repository. The whole songs these were
    /// checked against are compared by `fixtures_from_aksharamukha` when the
    /// local, gitignored fixture file is present.
    #[test]
    fn tamil_readable_voices_by_position() {
        let r = IndicStyle::Readable;
        assert_eq!(line("நிலாவே வா செல்லாதே வா", r), "nilaave vaa sellaadhe vaa");
        assert_eq!(line("காற்றும்", r), "kaatrum", "ற்ற is tr");
        assert_eq!(line("கூடாதென்று", r), "koodaadhendru", "ன்ற is ndr");
        assert_eq!(line("தங்காதம்மா நெஞ்சம்", r), "thangaadhammaa nenjam");
        assert_eq!(line("வைத்தாய்", r), "vaiththaay", "doubled stops stay voiceless");
        assert_eq!(line("பூஞ்சோலையில்", r), "poonjolaiyil", "ச after ஞ் is j");
        assert_eq!(line("மதம் பேசு", r), "madham pesu", "Aksharamukha's own example");
    }

    #[test]
    fn tamil_iso_is_strict() {
        let i = IndicStyle::Iso;
        assert_eq!(line("நிலாவே வா செல்லாதே வா", i), "nilāvē vā cellātē vā");
        assert_eq!(line("என்னாளும்", i), "eṉṉāḷum");
    }

    #[test]
    fn hindi_readable_is_lyric_spelling() {
        let r = IndicStyle::Readable;
        assert_eq!(line("तुम पास आये, यूँ मुस्कुराये", r), "tum paas aaye, yun muskuraaye");
        assert_eq!(line("क्या करूँ हाय, कुछ कुछ होता है", r), "kya karun haay, kuchh kuchh hota hai");
        assert_eq!(line("बुझती नहीं है", r), "bujhti nahin hai");
        assert_eq!(
            line("ये इश्क़ जाने कैसे हुआ", r),
            "ye ishq jaane kaise hua",
            "a nukta letter loses its final schwa too"
        );
        assert_eq!(line("तनहाई में दिल", r), "tanhaai mein dil", "medial schwa deletion");
        assert_eq!(line("रंग", r), "rang");
        assert_eq!(line("समझना", r), "samajhna", "deletion right to left");
        assert_eq!(line("ज्ञान", r), "gyaan");
        assert_eq!(line("तुम पास आए, यूँ मुस्कुराए", r), "tum paas aaye, yun muskuraaye", "ए after ā");
    }

    #[test]
    fn hindi_iso_keeps_every_schwa() {
        assert_eq!(line("तुम पास आये", IndicStyle::Iso), "tuma pāsa āyē");
        assert_eq!(line("नहीं", IndicStyle::Iso), "nahīṁ");
    }

    #[test]
    fn the_other_scripts_come_through_the_same_tables() {
        let r = IndicStyle::Readable;
        assert_eq!(line("നന്ദി", r), "nandi", "Malayalam");
        assert_eq!(line("ಕನ್ನಡ", r), "kannada", "Kannada");
        assert_eq!(line("తెలుగు", r), "telugu", "Telugu");
        assert_eq!(line("ਪੰਜਾਬ", r), "panjaab", "Gurmukhi, tippi and final schwa");
        assert_eq!(line("ગુજરાત", r), "gujaraat", "Gujarati, final schwa");
        assert_eq!(line("আমার", r), "aamaar", "Bengali, final schwa");
        assert_eq!(line("അവൻ", r), "avan", "Malayalam chillu");
    }

    /// The whole of the sample songs against Aksharamukha's own output, when
    /// the fixture file generated in a scratch environment is present
    /// (`temp/romanizer/ak_fixtures.json`, gitignored: it holds lyrics).
    /// Tamil readable must match it with the apostrophes taken out; ISO must
    /// match outright for both.
    #[test]
    #[ignore = "needs the local, gitignored Aksharamukha fixture file"]
    fn fixtures_from_aksharamukha() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/temp/romanizer/ak_fixtures.json");
        let Ok(raw) = std::fs::read_to_string(path) else {
            eprintln!("no fixture file at {path}");
            return;
        };
        let recs: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap();
        let mut bad = Vec::new();
        for r in recs {
            let input = r["input"].as_str().unwrap().trim_end_matches([' ', '.']);
            if input.trim().is_empty() {
                continue;
            }
            let tamil = r["script"] == "Tamil";
            let want_iso = r["iso"].as_str().unwrap().trim_end_matches([' ', '.']).to_string();
            let got_iso = line(input, IndicStyle::Iso);
            if got_iso != want_iso {
                bad.push(format!("ISO {input}\n  want {want_iso}\n  got  {got_iso}"));
            }
            if tamil {
                let want = plain(r["readable"].as_str().unwrap());
                let got = line(input, IndicStyle::Readable);
                if got != want {
                    bad.push(format!("readable {input}\n  want {want}\n  got  {got}"));
                }
            }
        }
        assert!(bad.is_empty(), "{} mismatches:\n{}", bad.len(), bad.join("\n"));
    }
}
