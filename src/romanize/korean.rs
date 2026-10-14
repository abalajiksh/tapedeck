//! Hangul in Latin letters, as it is **sung**.
//!
//! Revised Romanization's letters, applied after the standard pronunciation
//! rules (표준 발음법) have resyllabified the word: a final consonant moving onto
//! a following vowel, ㅎ aspirating its neighbour, nasal and liquid
//! assimilation, palatalization, and — beyond what RR itself writes —
//! tensification, because the point is singing along (marked by the stop
//! before it rather than by doubling: 잡지 is jap·ji). So 꽃향기만 is
//! `kko·tyang·gi·man`, not `kkot·hyang·gi·man`: the syllables are the ones a
//! singer makes, and there is still exactly one per Hangul block, which is what
//! lets the reader set each under its block.
//!
//! Ported from the rules rather than from korean-romanizer-go (GPL-3.0, itself
//! a port of osori/korean-romanizer), whose pronouncer has four bugs this does
//! not copy: its ㅎ rule tests the next syllable's *final* where it means its
//! initial, its double-final table is keyed on the wrong jamo block and never
//! matches, it neutralizes before liaison (옷이 → "odi"), and it has no nasal
//! or liquid assimilation at all, so 합니다 came out "hapnida".

use super::{Part, Word};

/// The consonants, by sound rather than by jamo, so a cluster is just two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum C {
    G,
    Kk,
    N,
    D,
    Tt,
    R,
    M,
    B,
    Pp,
    S,
    Ss,
    J,
    Jj,
    Ch,
    K,
    T,
    P,
    H,
    /// ㅇ as a final, the only place it is a sound.
    Ng,
}

/// Initial consonants in jamo order; ㅇ (index 11) is no consonant at all.
const INITIAL: [Option<C>; 19] = [
    Some(C::G),
    Some(C::Kk),
    Some(C::N),
    Some(C::D),
    Some(C::Tt),
    Some(C::R),
    Some(C::M),
    Some(C::B),
    Some(C::Pp),
    Some(C::S),
    Some(C::Ss),
    None,
    Some(C::J),
    Some(C::Jj),
    Some(C::Ch),
    Some(C::K),
    Some(C::T),
    Some(C::P),
    Some(C::H),
];

const VOWEL: [&str; 21] = [
    "a", "ae", "ya", "yae", "eo", "e", "yeo", "ye", "o", "wa", "wae", "oe", "yo", "u", "wo", "we",
    "wi", "yu", "eu", "ui", "i",
];
/// ㅣ, for palatalization.
const VOWEL_I: usize = 20;

/// Finals in jamo order (index 0 is none), clusters as their two sounds.
fn finals(i: u32) -> Vec<C> {
    use C::*;
    match i {
        1 => vec![G],
        2 => vec![Kk],
        3 => vec![G, S],
        4 => vec![N],
        5 => vec![N, J],
        6 => vec![N, H],
        7 => vec![D],
        8 => vec![R],
        9 => vec![R, G],
        10 => vec![R, M],
        11 => vec![R, B],
        12 => vec![R, S],
        13 => vec![R, T],
        14 => vec![R, P],
        15 => vec![R, H],
        16 => vec![M],
        17 => vec![B],
        18 => vec![B, S],
        19 => vec![S],
        20 => vec![Ss],
        21 => vec![Ng],
        22 => vec![J],
        23 => vec![Ch],
        24 => vec![K],
        25 => vec![T],
        26 => vec![P],
        27 => vec![H],
        _ => vec![],
    }
}

#[derive(Debug, Clone)]
struct Syl {
    block: char,
    onset: Option<C>,
    vowel: usize,
    coda: Vec<C>,
}

fn decompose(c: char) -> Option<Syl> {
    let u = c as u32;
    if !(0xAC00..=0xD7A3).contains(&u) {
        return None;
    }
    let i = u - 0xAC00;
    Some(Syl {
        block: c,
        onset: INITIAL[(i / 588) as usize],
        vowel: ((i % 588) / 28) as usize,
        coda: finals(i % 28),
    })
}

fn aspirated(c: C) -> Option<C> {
    match c {
        C::G | C::Kk | C::K => Some(C::K),
        C::D | C::T | C::S | C::Ss => Some(C::T),
        C::J | C::Ch => Some(C::Ch),
        C::B | C::P => Some(C::P),
        _ => None,
    }
}

fn tensed(c: C) -> Option<C> {
    match c {
        C::G => Some(C::Kk),
        C::D => Some(C::Tt),
        C::B => Some(C::Pp),
        C::S => Some(C::Ss),
        C::J => Some(C::Jj),
        _ => None,
    }
}

/// The seven sounds a syllable may end on, for what is left in final position.
fn neutral(coda: &[C]) -> Option<C> {
    use C::*;
    match coda {
        [] => None,
        [G, S] | [R, G] => Some(G),
        [N, J] | [N, H] => Some(N),
        [R, M] => Some(M),
        [R, P] | [B, S] => Some(B),
        [R, B] | [R, S] | [R, T] | [R, H] => Some(R),
        [one] => Some(match one {
            G | Kk | K => G,
            D | Tt | T | S | Ss | J | Jj | Ch | H => D,
            B | Pp | P => B,
            other => *other,
        }),
        _ => None,
    }
}

/// The pronunciation rules across one syllable boundary, in the order the
/// standard applies them. Mutates the final of `a` and the initial of `b`.
fn boundary(a: &mut Syl, b: &mut Syl) {
    // ㅎ as a final: it aspirates a following ㄱ ㄷ ㅈ, doubles ㅅ, is silent
    // before a vowel, and becomes ㄴ before ㄴ (좋고 → 조코, 좋은 → 조은, 놓는 → 논는).
    if a.coda.last() == Some(&C::H) {
        a.coda.pop();
        match b.onset {
            Some(C::G) | Some(C::D) | Some(C::J) => b.onset = b.onset.and_then(aspirated),
            Some(C::S) => b.onset = Some(C::Ss),
            Some(C::N) if a.coda.is_empty() => a.coda.push(C::N),
            _ => {}
        }
    }
    // A stop before ㅎ is aspirated by it and leaves the final empty:
    // 축하 → 추카, 꽃향기 → 꼬턍기.
    if b.onset == Some(C::H) {
        if let Some(&last) = a.coda.last() {
            let place = neutral(&[last]).and_then(aspirated);
            if let (Some(asp), true) = (place, !matches!(last, C::N | C::M | C::R | C::Ng)) {
                // ㅈ + ㅎ is ㅊ (맞히다 → 마치다); ㅊ itself has already
                // neutralized to ㄷ and gives ㅌ (꽃향기 → 꼬턍기).
                let asp = if last == C::J { C::Ch } else { asp };
                a.coda.pop();
                b.onset = Some(asp);
            }
        }
    }
    // ㄺ before ㄱ keeps the ㄹ and tenses the ㄱ (붉게 → 불께, 맑게 → 말께);
    // everywhere else it is ㄱ.
    if a.coda == [C::R, C::G] && b.onset == Some(C::G) {
        a.coda = vec![C::R];
        b.onset = Some(C::Kk);
    }
    // Liaison: a final moves onto a following vowel, and of a cluster only the
    // second half does (옷이 → 오시, 없어 → 업서). ㄷ ㅌ before 이 palatalize
    // (같이 → 가치). ㅇ never moves.
    if b.onset.is_none() && !a.coda.is_empty() && a.coda != [C::Ng] {
        let moved = a.coda.pop().expect("checked non-empty");
        b.onset = Some(match (moved, b.vowel == VOWEL_I) {
            (C::D, true) => C::J,
            (C::T, true) => C::Ch,
            (other, _) => other,
        });
    }
    // What is still in final position is one of seven sounds.
    let coda = neutral(&a.coda);
    a.coda = coda.into_iter().collect();

    // Assimilation, now that both sides are settled.
    match (coda, b.onset) {
        // Nasalization: a stop before ㄴ ㅁ (국물 → 궁물, 합니다 → 함니다).
        (Some(C::G), Some(C::N | C::M)) => a.coda = vec![C::Ng],
        (Some(C::D), Some(C::N | C::M)) => a.coda = vec![C::N],
        (Some(C::B), Some(C::N | C::M)) => a.coda = vec![C::M],
        // ㄹ after ㅁ ㅇ is ㄴ (종로 → 종노); after a stop, both shift (독립 → 동닙).
        (Some(C::M | C::Ng), Some(C::R)) => b.onset = Some(C::N),
        (Some(C::G), Some(C::R)) => {
            a.coda = vec![C::Ng];
            b.onset = Some(C::N);
        }
        (Some(C::B), Some(C::R)) => {
            a.coda = vec![C::M];
            b.onset = Some(C::N);
        }
        // Liquidization: ㄴ and ㄹ together are both ㄹ (신라 → 실라, 설날 → 설랄).
        (Some(C::N), Some(C::R)) => a.coda = vec![C::R],
        (Some(C::R), Some(C::N)) => b.onset = Some(C::R),
        // Tensification after a stop — sung, though RR does not write it
        // (학교 → 학꾜, 없이 → 업씨).
        (Some(C::G | C::D | C::B), Some(on)) => {
            if let Some(t) = tensed(on) {
                b.onset = Some(t);
            }
        }
        _ => {}
    }
}

fn onset_letters(onset: Option<C>, prev_coda: Option<C>) -> &'static str {
    let Some(c) = onset else { return "" };
    // After a stop the tensed consonant is written once: the stop already
    // makes it tense, so 갔단다 is gat·tan·da and 잡지 jap·ji — not "gatttanda"
    // or "japjji", which help nobody sing and which no lyric site writes. A
    // tensed consonant after a vowel (꽃 kkot, 싹 ssak) keeps its double.
    let after_stop = matches!(prev_coda, Some(C::G | C::D | C::B));
    match c {
        C::G => "g",
        C::Kk if after_stop => "k",
        C::Kk => "kk",
        C::N => "n",
        C::D => "d",
        C::Tt if after_stop => "t",
        C::Tt => "tt",
        // ㄹㄹ is ll.
        C::R if prev_coda == Some(C::R) => "l",
        C::R => "r",
        C::M => "m",
        C::B => "b",
        C::Pp if after_stop => "p",
        C::Pp => "pp",
        C::S | C::Ss if after_stop => "s",
        C::S => "s",
        C::Ss => "ss",
        C::J | C::Jj if after_stop => "j",
        C::J => "j",
        C::Jj => "jj",
        C::Ch => "ch",
        C::K => "k",
        C::T => "t",
        C::P => "p",
        C::H => "h",
        C::Ng => "ng",
    }
}

fn coda_letters(coda: Option<C>) -> &'static str {
    match coda {
        Some(C::G) => "k",
        Some(C::N) => "n",
        Some(C::D) => "t",
        Some(C::R) => "l",
        Some(C::M) => "m",
        Some(C::B) => "p",
        Some(C::Ng) => "ng",
        _ => "",
    }
}

/// One word: a run of Hangul blocks with no space in it.
///
/// The rules apply inside a word only. Across a space RR keeps words apart and
/// so does this — a singer does link them, but a romanization that ran words
/// together would be harder to read, which is the whole complaint.
pub(super) fn word(run: &str) -> Word {
    let mut syls: Vec<Option<Syl>> = run.chars().map(decompose).collect();
    for i in 0..syls.len().saturating_sub(1) {
        let (left, right) = syls.split_at_mut(i + 1);
        if let (Some(a), Some(b)) = (left[i].as_mut(), right[0].as_mut()) {
            boundary(a, b);
        }
    }
    // The last block's final is neutralized too (밖 → bak).
    if let Some(Some(last)) = syls.last_mut() {
        last.coda = neutral(&last.coda).into_iter().collect();
    }

    let mut parts = Vec::with_capacity(syls.len());
    let mut prev_coda: Option<C> = None;
    for (s, c) in syls.iter().zip(run.chars()) {
        match s {
            Some(s) => {
                let coda = s.coda.first().copied();
                let r = format!(
                    "{}{}{}",
                    onset_letters(s.onset, prev_coda),
                    VOWEL[s.vowel],
                    coda_letters(coda)
                );
                parts.push(Part { o: s.block.to_string(), r, lit: false });
                prev_coda = coda;
            }
            // A run is Hangul syllables only, so this is unreachable in
            // practice; carried as written if it ever is not.
            None => {
                parts.push(Part { o: c.to_string(), r: c.to_string(), lit: true });
                prev_coda = None;
            }
        }
    }
    Word { parts, syl: true }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sung(s: &str) -> String {
        s.split(' ')
            .map(|w| word(w).parts.iter().map(|p| p.r.as_str()).collect::<Vec<_>>().join("·"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The reference implementation's own tests, which still hold.
    #[test]
    fn the_reference_cases() {
        assert_eq!(sung("안녕하세요"), "an·nyeong·ha·se·yo");
        assert_eq!(sung("아이유 방탄소년단"), "a·i·yu bang·tan·so·nyeon·dan");
        assert_eq!(sung("구미"), "gu·mi");
        assert_eq!(sung("영동"), "yeong·dong");
        assert_eq!(sung("한밭"), "han·bat");
        assert_eq!(sung("설악"), "seo·rak");
        assert_eq!(sung("강약"), "gang·yak");
        assert_eq!(sung("좋아하고"), "jo·a·ha·go");
        assert_eq!(sung("좋은"), "jo·eun");
    }

    /// What the reference got wrong, each a rule it skipped or misapplied.
    #[test]
    fn the_rules_the_reference_missed() {
        assert_eq!(sung("감사합니다"), "gam·sa·ham·ni·da", "nasalization");
        assert_eq!(sung("국물"), "gung·mul", "nasalization");
        assert_eq!(sung("좋고"), "jo·ko", "ㅎ aspirates the next initial");
        assert_eq!(sung("옷이"), "o·si", "liaison before neutralization");
        assert_eq!(sung("같이"), "ga·chi", "palatalization");
        assert_eq!(sung("신라"), "sil·la", "liquidization");
        assert_eq!(sung("설날"), "seol·lal", "liquidization");
        assert_eq!(sung("독립"), "dong·nip", "ㄹ after a stop");
        assert_eq!(sung("종로"), "jong·no", "ㄹ after ㅇ");
    }

    /// 꽃 (FLOWER), the song that asked for this: the syllables a singer makes.
    #[test]
    fn as_sung() {
        assert_eq!(sung("꽃향기만 남기고 갔단다"), "kko·tyang·gi·man nam·gi·go gat·tan·da");
        assert_eq!(sung("괜찮아"), "gwaen·cha·na", "ㄶ before a vowel");
        assert_eq!(sung("없이"), "eop·si", "cluster liaison; tensed after a stop, written once");
        assert_eq!(sung("변했지"), "byeon·haet·ji");
        assert_eq!(sung("학교"), "hak·kyo", "tensed after a stop, written once");
        assert_eq!(sung("했어"), "hae·sseo");
        assert_eq!(sung("몰라"), "mol·la");
        assert_eq!(sung("붉게"), "bul·kke", "ㄺ before ㄱ keeps the ㄹ");
        assert_eq!(sung("밝다"), "bak·ta", "and is ㄱ before anything else");
        assert_eq!(sung("밖"), "bak", "a final is neutralized at the end too");
        assert_eq!(sung("의미"), "ui·mi", "ㅢ is ui, as RR writes it");
    }

    #[test]
    fn one_part_per_block() {
        let w = word("꽃향기만");
        assert_eq!(w.parts.iter().map(|p| p.o.as_str()).collect::<String>(), "꽃향기만");
        assert!(w.syl);
    }
}
