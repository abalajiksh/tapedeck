//! Mandarin in pinyin with tone marks.
//!
//! Words come from lindera with CC-CEDICT embedded, which reads a word right
//! and a lone character often wrong: a character that is a word on its own
//! gets whichever of its readings the dictionary lists first. Measured on a
//! real lyric (晴天) that was 的 as dì seventeen times, 着 as zhuó, 了 as liào,
//! 那 as nuó, 雨 as yù. So a lone character takes, in order: `ALONE` below
//! (the particles at their neutral tone, which no frequency list gets
//! right), then its most common reading from the `pinyin` crate (MIT, from
//! pinyin-data, ordered by frequency), and only then the dictionary's.

use std::borrow::Cow;
use std::sync::OnceLock;

use lindera::dictionary::load_dictionary;
use lindera::mode::Mode;
use lindera::segmenter::Segmenter;

use super::{Part, Word};

/// CC-CEDICT's pinyin field: numbered tones, space-separated ("yin2 hang2").
const PINYIN: usize = 4;

/// Readings for a character standing alone, simplified and traditional. Tone
/// 5 is the neutral tone.
const ALONE: &[(char, &str)] = &[
    // Particles and suffixes, neutral.
    ('的', "de5"),
    ('地', "de5"),
    ('得', "de5"),
    ('了', "le5"),
    ('着', "zhe5"),
    ('著', "zhe5"),
    ('么', "me5"),
    ('麼', "me5"),
    ('吗', "ma5"),
    ('嗎', "ma5"),
    ('呢', "ne5"),
    ('吧', "ba5"),
    ('啊', "a5"),
    ('呀', "ya5"),
    ('啦', "la5"),
    ('嘛', "ma5"),
    ('哦', "o5"),
    ('们', "men5"),
    ('們', "men5"),
    ('过', "guo4"),
    ('過', "guo4"),
    ('个', "ge4"),
    ('個', "ge4"),
    // Everyday polyphones at their everyday reading.
    ('那', "na4"),
    ('哪', "na3"),
    ('还', "hai2"),
    ('還', "hai2"),
    ('行', "xing2"),
    ('长', "chang2"),
    ('長', "chang2"),
    ('重', "zhong4"),
    ('为', "wei4"),
    ('為', "wei4"),
    ('只', "zhi3"),
    ('都', "dou1"),
    ('没', "mei2"),
    ('沒', "mei2"),
    ('和', "he2"),
    ('曲', "qu3"),
    ('便', "bian4"),
    ('种', "zhong3"),
    ('種', "zhong3"),
    ('大', "da4"),
    ('说', "shuo1"),
    ('說', "shuo1"),
    ('要', "yao4"),
    ('会', "hui4"),
    ('會', "hui4"),
    ('中', "zhong1"),
    ('相', "xiang1"),
    ('间', "jian1"),
    ('間', "jian1"),
    ('少', "shao3"),
    ('正', "zheng4"),
    ('好', "hao3"),
    ('看', "kan4"),
    ('教', "jiao1"),
    ('觉', "jue2"),
    ('覺', "jue2"),
    ('发', "fa1"),
    ('發', "fa1"),
    ('落', "luo4"),
    ('当', "dang1"),
    ('當', "dang1"),
    ('数', "shu4"),
    ('數', "shu4"),
    ('空', "kong1"),
    ('更', "geng4"),
    ('差', "cha4"),
    ('似', "si4"),
    ('处', "chu4"),
    ('處', "chu4"),
    ('难', "nan2"),
    ('難', "nan2"),
    ('应', "ying1"),
    ('應', "ying1"),
    ('什', "shen2"),
    ('一', "yi1"),
    ('不', "bu4"),
    ('乐', "le4"),
    ('樂', "le4"),
    ('调', "diao4"),
    ('調', "diao4"),
    ('弹', "tan2"),
    ('彈', "tan2"),
    ('转', "zhuan3"),
    ('轉', "zhuan3"),
    ('朝', "chao2"),
    ('传', "chuan2"),
    ('傳', "chuan2"),
    ('载', "zai4"),
    ('載', "zai4"),
    ('背', "bei4"),
    ('薄', "bao2"),
    ('露', "lu4"),
    ('血', "xue4"),
    ('解', "jie3"),
];

fn segmenter() -> Option<&'static Segmenter> {
    static SEG: OnceLock<Option<Segmenter>> = OnceLock::new();
    SEG.get_or_init(|| match load_dictionary("embedded://cc-cedict") {
        Ok(d) => Some(Segmenter::new(Mode::Normal, d, None)),
        Err(e) => {
            tracing::warn!("🎤 Chinese dictionary unavailable, hanzi will be left as written: {e}");
            None
        }
    })
    .as_ref()
}

fn is_han(c: char) -> bool {
    matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF)
}

/// A character's most common reading, tone-marked.
fn common(c: char) -> Option<String> {
    use pinyin::ToPinyin;
    c.to_pinyin().map(|p| p.with_tone().to_string())
}

/// `hang2` → `háng`, `lu:4` → `lǜ`, `de5` → `de`. CC-CEDICT capitalizes a
/// proper name's syllables (Zhou1 Jie2 lun2), and the capital is kept.
pub(super) fn mark(numbered: &str) -> String {
    let capital = numbered.chars().next().is_some_and(char::is_uppercase);
    let marked = mark_lower(numbered);
    if capital {
        let mut c = marked.chars();
        c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or(marked)
    } else {
        marked
    }
}

fn mark_lower(numbered: &str) -> String {
    let s = numbered.to_lowercase().replace("u:", "ü").replace('v', "ü");
    let (body, tone) = match s.chars().last().and_then(|c| c.to_digit(10)) {
        Some(t) => (&s[..s.len() - 1], t),
        None => (s.as_str(), 5),
    };
    if !(1..=4).contains(&tone) {
        return body.to_string();
    }
    let chars: Vec<char> = body.chars().collect();
    // a or e takes the mark; in ou it is the o; otherwise the last vowel.
    let at = chars
        .iter()
        .position(|c| *c == 'a' || *c == 'e')
        .or_else(|| body.find("ou").map(|_| chars.iter().position(|c| *c == 'o').unwrap_or(0)))
        .or_else(|| chars.iter().rposition(|c| "iouü".contains(*c)));
    let Some(at) = at else { return body.to_string() };
    let marked = |v: char| -> char {
        let row = match v {
            'a' => ['ā', 'á', 'ǎ', 'à'],
            'e' => ['ē', 'é', 'ě', 'è'],
            'i' => ['ī', 'í', 'ǐ', 'ì'],
            'o' => ['ō', 'ó', 'ǒ', 'ò'],
            'u' => ['ū', 'ú', 'ǔ', 'ù'],
            'ü' => ['ǖ', 'ǘ', 'ǚ', 'ǜ'],
            other => return other,
        };
        row[(tone - 1) as usize]
    };
    chars.iter().enumerate().map(|(i, c)| if i == at { marked(*c) } else { *c }).collect()
}

/// Full-width punctuation as it is usually written in pinyin text.
fn punct(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '，' | '、' => ',',
            '。' => '.',
            '！' => '!',
            '？' => '?',
            '：' => ':',
            '；' => ';',
            '“' | '”' | '「' | '」' | '『' | '』' => '"',
            c if ('\u{FF01}'..='\u{FF5E}').contains(&c) => {
                char::from_u32(c as u32 - 0xFEE0).unwrap_or(c)
            }
            c => c,
        })
        .collect()
}

pub(super) fn words(run: &str) -> Vec<Word> {
    let Some(seg) = segmenter() else {
        return vec![Word {
            parts: vec![Part { o: run.to_string(), r: run.to_string(), lit: true }],
            syl: false,
        }];
    };
    let Ok(mut toks) = seg.segment(Cow::Borrowed(run)) else {
        return Vec::new();
    };
    let mut out: Vec<Word> = Vec::new();
    for t in toks.iter_mut() {
        let surface = t.surface.to_string();
        let chars: Vec<char> = surface.chars().collect();
        if !chars.iter().any(|c| is_han(*c)) {
            // Punctuation attaches to the word before it; Latin stands alone.
            let part = Part { o: surface.clone(), r: punct(&surface), lit: true };
            match out.last_mut() {
                Some(w) if !surface.chars().any(char::is_alphanumeric) => w.parts.push(part),
                _ => out.push(Word { parts: vec![part], syl: false }),
            }
            continue;
        }
        let dict: Vec<String> = t
            .details()
            .get(PINYIN)
            .map(|p| p.split_whitespace().map(str::to_string).collect())
            .unwrap_or_default();
        let lone = chars.len() == 1;
        let parts: Vec<Part> = chars
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let r: Option<String> = if lone {
                    ALONE
                        .iter()
                        .find(|(k, _)| k == c)
                        .map(|(_, p)| mark(p))
                        .or_else(|| common(*c))
                        .or_else(|| dict.first().map(|p| mark(p)))
                } else if dict.len() == chars.len() {
                    dict.get(i).map(|p| mark(p))
                } else {
                    common(*c)
                };
                let found = r.is_some();
                let mut r = r.unwrap_or_else(|| c.to_string());
                // xī'ān, not xīān: a syllable opening on a, o or e inside a word
                // is set off from the one before it.
                if i > 0
                    && r.starts_with([
                        'a', 'o', 'e', 'ā', 'á', 'ǎ', 'à', 'ō', 'ó', 'ǒ', 'ò', 'ē', 'é', 'ě', 'è',
                    ])
                {
                    r.insert(0, '\'');
                }
                Part { o: c.to_string(), r, lit: !found }
            })
            .collect();
        // CC-CEDICT capitalizes each part of a name (Zhou1 Jie2 lun2): a
        // capital after the first syllable starts a new word, so 周杰伦 is
        // "Zhōu Jiélún" rather than "ZhōuJiélún".
        let mut word: Vec<Part> = Vec::new();
        for p in parts {
            if !word.is_empty() && p.r.chars().next().is_some_and(char::is_uppercase) {
                out.push(Word { parts: std::mem::take(&mut word), syl: false });
            }
            word.push(p);
        }
        out.push(Word { parts: word, syl: false });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> String {
        words(s)
            .iter()
            .map(|w| w.parts.iter().map(|p| p.r.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn tone_marks_go_where_pinyin_puts_them() {
        assert_eq!(mark("hang2"), "háng");
        assert_eq!(mark("hao3"), "hǎo");
        assert_eq!(mark("dou1"), "dōu");
        assert_eq!(mark("gui4"), "guì");
        assert_eq!(mark("lu:4"), "lǜ");
        assert_eq!(mark("de5"), "de");
        assert_eq!(mark("Zhou1"), "Zhōu", "a proper name keeps its capital");
    }

    /// Constructed sentences, each a reading the dictionary alone gets wrong.
    #[test]
    fn lone_characters_take_their_common_reading() {
        assert_eq!(text("我爱你"), "wǒ ài nǐ");
        assert_eq!(text("我们一起走了"), "wǒmen yīqǐ zǒu le");
        // 那里 is a word, read nàli with its neutral second syllable.
        assert_eq!(text("银行在那里"), "yínháng zài nàli");
        assert_eq!(text("你还要多久"), "nǐ hái yào duōjiǔ");
    }

    #[test]
    fn a_lone_character_reads_as_it_most_often_does() {
        // CC-CEDICT lists yù (to rain) and lìn (to filter) first.
        assert_eq!(text("下雨"), "xiàyǔ");
        assert_eq!(text("我好想再淋一遍"), "wǒ hǎo xiǎng zài lín yībiàn");
    }

    #[test]
    fn a_name_is_written_as_its_parts() {
        assert_eq!(text("周杰伦"), "Zhōu Jiélún");
    }

    #[test]
    fn traditional_characters_read_the_same() {
        assert_eq!(text("還要多久"), "hái yào duōjiǔ");
    }
}
