//! Russian in Latin letters.
//!
//! The schemes are iuliia's (Anton Zhiyanov, MIT): a letter table plus three
//! kinds of context rule — a letter read differently after another (`е` is
//! `ye` at the start of a word and after a vowel or a hard or soft sign), a
//! letter read differently before another (`ъ`/`ь` before a vowel is `y`), and
//! a word ending read as a whole (`-ий`, `-ый` → `-y`). The tables are copied
//! from iuliia's `wikipedia.json` and `gost_779.json`, and their samples are
//! the tests.
//!
//! **Spelled, not sung.** Korean is romanized as it is pronounced; Russian is
//! romanized as it is written. Reading it as sung — unstressed `о` as `a`, a
//! `ё` written as `е` — needs to know where the stress falls, which is a
//! dictionary rather than a rule, and lyric sites spell it as written too.
//!
//! **Russian only.** Ukrainian, Belarusian, Serbian, Bulgarian and the rest
//! share the alphabet and read it differently (Bulgarian `ъ` is a vowel, which
//! these tables drop). [`not_russian`] spots the letters only those languages
//! have, and a lyric that has them is left as written rather than romanized
//! wrongly.

use super::{CyrillicStyle, Part, Word};

/// One scheme: the letter table and its context rules. Keys are lower case.
struct Scheme {
    letters: &'static [(char, &'static str)],
    /// `(previous, letter)` → reading. A previous of `None` is the start of a
    /// word.
    after: &'static [(Option<char>, char, &'static str)],
    /// `(letter, next)` → reading of the letter.
    before: &'static [(char, char, &'static str)],
    /// A word's last two letters → their reading together.
    endings: &'static [(&'static str, &'static str)],
}

/// Wikipedia's scheme: no diacritics, the spelling an English-language page
/// (and a lyric site) uses. Юлия → Yuliya, Ельцин → Yeltsin, Красный → Krasny.
const WIKIPEDIA: Scheme = Scheme {
    letters: &[
        ('а', "a"),
        ('б', "b"),
        ('в', "v"),
        ('г', "g"),
        ('д', "d"),
        ('е', "e"),
        ('ё', "yo"),
        ('ж', "zh"),
        ('з', "z"),
        ('и', "i"),
        ('й', "y"),
        ('к', "k"),
        ('л', "l"),
        ('м', "m"),
        ('н', "n"),
        ('о', "o"),
        ('п', "p"),
        ('р', "r"),
        ('с', "s"),
        ('т', "t"),
        ('у', "u"),
        ('ф', "f"),
        ('х', "kh"),
        ('ц', "ts"),
        ('ч', "ch"),
        ('ш', "sh"),
        ('щ', "shch"),
        ('ъ', ""),
        ('ы', "y"),
        ('ь', ""),
        ('э', "e"),
        ('ю', "yu"),
        ('я', "ya"),
    ],
    after: &[
        (None, 'е', "ye"),
        (Some('а'), 'е', "ye"),
        (Some('и'), 'е', "ye"),
        (Some('о'), 'е', "ye"),
        (Some('у'), 'е', "ye"),
        (Some('э'), 'е', "ye"),
        (Some('ю'), 'е', "ye"),
        (Some('я'), 'е', "ye"),
        (Some('ь'), 'е', "ye"),
        (Some('ъ'), 'е', "ye"),
    ],
    before: &[
        ('ъ', 'а', "y"),
        ('ъ', 'и', "y"),
        ('ъ', 'о', "y"),
        ('ъ', 'у', "y"),
        ('ъ', 'ы', "y"),
        ('ъ', 'э', "y"),
        ('ь', 'а', "y"),
        ('ь', 'и', "y"),
        ('ь', 'о', "y"),
        ('ь', 'у', "y"),
        ('ь', 'ы', "y"),
        ('ь', 'э', "y"),
    ],
    endings: &[("ий", "y"), ("ый", "y")],
};

/// ISO 9:1995 (GOST 7.79-2000 System A): one Latin letter per Cyrillic one,
/// so it reads back exactly. Юлия → Ûliâ, щи → ŝi, съешь → sʺešʹ.
const ISO_9: Scheme = Scheme {
    letters: &[
        ('а', "a"),
        ('б', "b"),
        ('в', "v"),
        ('г', "g"),
        ('д', "d"),
        ('е', "e"),
        ('ё', "ë"),
        ('ж', "ž"),
        ('з', "z"),
        ('и', "i"),
        ('й', "j"),
        ('к', "k"),
        ('л', "l"),
        ('м', "m"),
        ('н', "n"),
        ('о', "o"),
        ('п', "p"),
        ('р', "r"),
        ('с', "s"),
        ('т', "t"),
        ('у', "u"),
        ('ф', "f"),
        ('х', "h"),
        ('ц', "c"),
        ('ч', "č"),
        ('ш', "š"),
        ('щ', "ŝ"),
        ('ъ', "ʺ"),
        ('ы', "y"),
        ('ь', "ʹ"),
        ('э', "è"),
        ('ю', "û"),
        ('я', "â"),
    ],
    after: &[],
    before: &[],
    endings: &[],
};

/// Letters of the Cyrillic block that Russian does not use and its neighbours
/// do: Ukrainian і ї є ґ, Belarusian ў, Serbian and Macedonian ђ ј љ њ ћ џ ѓ ќ
/// ѕ, and the Central Asian and Caucasian additions in the block's upper half.
pub fn not_russian(c: char) -> bool {
    let lower = c.to_lowercase().next().unwrap_or(c);
    matches!(
        lower,
        'і' | 'ї' | 'є' | 'ґ' | 'ў' | 'ђ' | 'ј' | 'љ' | 'њ' | 'ћ' | 'џ' | 'ѓ' | 'ќ' | 'ѕ'
    ) || ('\u{0460}'..='\u{04FF}').contains(&c) && !matches!(c, '\u{0483}'..='\u{0489}')
}

/// A run of Cyrillic letters, romanized as one word part.
pub fn word(run: &str, style: CyrillicStyle) -> Word {
    let scheme = match style {
        CyrillicStyle::Readable => &WIKIPEDIA,
        CyrillicStyle::Iso => &ISO_9,
    };
    Word {
        parts: vec![Part { o: run.to_string(), r: romanize(run, scheme), lit: false }],
        syl: false,
    }
}

fn romanize(run: &str, scheme: &Scheme) -> String {
    use unicode_normalization::UnicodeNormalization;
    // Stress marks (а́) are a reading aid on the original; the Latin letters
    // carry the letter, not the mark.
    let letters: Vec<char> =
        run.chars().filter(|c| !matches!(*c, '\u{0300}' | '\u{0301}')).collect();
    let cased: Vec<char> = letters.iter().filter(|c| c.is_alphabetic()).copied().collect();
    let shouting = cased.len() > 1 && cased.iter().all(|c| !c.is_lowercase());
    let lower: Vec<char> = letters.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect();

    // The ending is read whole when the scheme has a reading for it.
    let n = lower.len();
    let ending = (n > 2)
        .then(|| {
            let last: String = lower[n - 2..].iter().collect();
            scheme.endings.iter().find(|(e, _)| *e == last).map(|(_, r)| *r)
        })
        .flatten();
    let stem = if ending.is_some() { n - 2 } else { n };

    let mut out = String::new();
    for i in 0..stem {
        let (prev, cur, next) = (i.checked_sub(1).map(|j| lower[j]), lower[i], lower.get(i + 1));
        let reading = scheme
            .after
            .iter()
            .find(|(p, c, _)| *p == prev && *c == cur)
            .map(|(_, _, r)| *r)
            .or_else(|| {
                let next = next?;
                scheme.before.iter().find(|(c, x, _)| *c == cur && x == next).map(|(_, _, r)| *r)
            })
            .or_else(|| scheme.letters.iter().find(|(c, _)| *c == cur).map(|(_, r)| *r));
        let upper = letters[i].is_uppercase();
        match reading {
            Some(r) => push_cased(&mut out, r, upper, shouting),
            // Not in the table: a letter of another Cyrillic language inside
            // a Russian lyric. Carried as written rather than dropped.
            None => out.push(letters[i]),
        }
    }
    if let Some(r) = ending {
        push_cased(&mut out, r, letters[stem].is_uppercase(), shouting);
    }
    out.nfc().collect()
}

/// A reading in the case of the letter it reads: `Ю` → `Yu`, or `YU` inside a
/// word written in capitals.
fn push_cased(out: &mut String, reading: &str, upper: bool, shouting: bool) {
    if !upper {
        out.push_str(reading);
    } else if shouting {
        out.extend(reading.chars().flat_map(char::to_uppercase));
    } else {
        let mut chars = reading.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rom(s: &str, scheme: &Scheme) -> String {
        // Sentence-level, the way a lyric line arrives: words are runs of
        // Cyrillic letters, everything between them is carried.
        let mut out = String::new();
        let mut run = String::new();
        for c in s.chars().chain(std::iter::once(' ')) {
            if ('\u{0400}'..='\u{04FF}').contains(&c) || (c == '\u{0301}' && !run.is_empty()) {
                run.push(c);
            } else {
                if !run.is_empty() {
                    out.push_str(&romanize(&run, scheme));
                    run.clear();
                }
                out.push(c);
            }
        }
        out.pop();
        out
    }

    /// Every sample `wikipedia.json` ships, verbatim.
    #[test]
    fn wikipedia_matches_iuliias_samples() {
        for (ru, en) in [
            (
                "Юлия, съешь ещё этих мягких французских булок из Йошкар-Олы, да выпей алтайского чаю",
                "Yuliya, syesh yeshchyo etikh myagkikh frantsuzskikh bulok iz Yoshkar-Oly, da vypey altayskogo chayu",
            ),
            ("Россия, город Йошкар-Ола, улица Яна Крастыня", "Rossiya, gorod Yoshkar-Ola, ulitsa Yana Krastynya"),
            ("Ельцин", "Yeltsin"),
            ("Раздольное", "Razdolnoye"),
            ("Юрьев", "Yuryev"),
            ("Белкин", "Belkin"),
            ("Бийск", "Biysk"),
            ("Подъярский", "Podyarsky"),
            ("Мусийкъонгийкоте", "Musiykyongiykote"),
            ("Давыдов", "Davydov"),
            ("Усолье", "Usolye"),
            ("Выхухоль", "Vykhukhol"),
            ("Дальнегорск", "Dalnegorsk"),
            ("Ильинский", "Ilyinsky"),
            ("Красный", "Krasny"),
            ("Великий", "Veliky"),
            ("Набережные Челны", "Naberezhnye Chelny"),
        ] {
            assert_eq!(rom(ru, &WIKIPEDIA), en, "{ru}");
        }
    }

    /// `gost_779.json`'s sample.
    #[test]
    fn iso_9_matches_iuliias_sample() {
        assert_eq!(
            rom(
                "Юлия, съешь ещё этих мягких французских булок из Йошкар-Олы, да выпей алтайского чаю",
                &ISO_9
            ),
            "Ûliâ, sʺešʹ eŝë ètih mâgkih francuzskih bulok iz Joškar-Oly, da vypej altajskogo čaû"
        );
    }

    #[test]
    fn capitals_and_stress_marks() {
        assert_eq!(rom("ЮЛИЯ", &WIKIPEDIA), "YULIYA");
        assert_eq!(rom("ЩИ", &ISO_9), "ŜI");
        assert_eq!(rom("голова́", &WIKIPEDIA), "golova");
    }

    #[test]
    fn other_cyrillic_languages_are_spotted() {
        assert!("Київ".chars().any(not_russian));
        assert!("Ђорђе".chars().any(not_russian));
        assert!(!"Мой мармеладный, я не права".chars().any(not_russian));
    }
}
