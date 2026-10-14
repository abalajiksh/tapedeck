// @generated from Aksharamukha's script_mapping.json (virtualvinodh/aksharamukha,
// commit d1fe162, AGPL-3.0) — the positional table its own engine maps through.
// Only single-character entries are kept; nukta letters, which NFC leaves as
// base + nukta, are in the `*_NUKTA` tables keyed on the base. `readable` is
// Aksharamukha's "Roman (Readable)" with its apostrophe markers left in; the
// romanizer strips them. Regenerate rather than edit by hand.

use super::indic::{Kind, Sym};

pub(super) static DEVANAGARI: &[Sym] = &[
    Sym { c: '\u{0905}', kind: Kind::Vowel, iso: "a", readable: "a" }, // अ
    Sym { c: '\u{0906}', kind: Kind::Vowel, iso: "ā", readable: "aa" }, // आ
    Sym { c: '\u{0907}', kind: Kind::Vowel, iso: "i", readable: "i" }, // इ
    Sym { c: '\u{0908}', kind: Kind::Vowel, iso: "ī", readable: "ee" }, // ई
    Sym { c: '\u{0909}', kind: Kind::Vowel, iso: "u", readable: "u" }, // उ
    Sym { c: '\u{090a}', kind: Kind::Vowel, iso: "ū", readable: "oo" }, // ऊ
    Sym { c: '\u{090b}', kind: Kind::Vowel, iso: "r̥", readable: "ri'" }, // ऋ
    Sym { c: '\u{0960}', kind: Kind::Vowel, iso: "r̥̄", readable: "ree'" }, // ॠ
    Sym { c: '\u{090c}', kind: Kind::Vowel, iso: "l̥", readable: "li'" }, // ऌ
    Sym { c: '\u{0961}', kind: Kind::Vowel, iso: "l̥̄", readable: "lee'" }, // ॡ
    Sym { c: '\u{090f}', kind: Kind::Vowel, iso: "ē", readable: "e" }, // ए
    Sym { c: '\u{0910}', kind: Kind::Vowel, iso: "ai", readable: "ai" }, // ऐ
    Sym { c: '\u{0913}', kind: Kind::Vowel, iso: "ō", readable: "o" }, // ओ
    Sym { c: '\u{0914}', kind: Kind::Vowel, iso: "au", readable: "au" }, // औ
    Sym { c: '\u{090e}', kind: Kind::Vowel, iso: "e", readable: "e'" }, // ऎ
    Sym { c: '\u{0912}', kind: Kind::Vowel, iso: "o", readable: "o'" }, // ऒ
    Sym { c: '\u{090d}', kind: Kind::Vowel, iso: "æ", readable: "a" }, // ऍ
    Sym { c: '\u{0911}', kind: Kind::Vowel, iso: "ô", readable: "o" }, // ऑ
    Sym { c: '\u{093e}', kind: Kind::Sign, iso: "ā", readable: "aa" }, // ा
    Sym { c: '\u{093f}', kind: Kind::Sign, iso: "i", readable: "i" },  // ि
    Sym { c: '\u{0940}', kind: Kind::Sign, iso: "ī", readable: "ee" }, // ी
    Sym { c: '\u{0941}', kind: Kind::Sign, iso: "u", readable: "u" },  // ु
    Sym { c: '\u{0942}', kind: Kind::Sign, iso: "ū", readable: "oo" }, // ू
    Sym { c: '\u{0943}', kind: Kind::Sign, iso: "r̥", readable: "ri'" }, // ृ
    Sym { c: '\u{0944}', kind: Kind::Sign, iso: "r̥̄", readable: "ree'" }, // ॄ
    Sym { c: '\u{0962}', kind: Kind::Sign, iso: "l̥", readable: "li'" }, // ॢ
    Sym { c: '\u{0963}', kind: Kind::Sign, iso: "l̥̄", readable: "lee'" }, // ॣ
    Sym { c: '\u{0947}', kind: Kind::Sign, iso: "ē", readable: "e" },  // े
    Sym { c: '\u{0948}', kind: Kind::Sign, iso: "ai", readable: "ai" }, // ै
    Sym { c: '\u{094b}', kind: Kind::Sign, iso: "ō", readable: "o" },  // ो
    Sym { c: '\u{094c}', kind: Kind::Sign, iso: "au", readable: "au" }, // ौ
    Sym { c: '\u{0946}', kind: Kind::Sign, iso: "e", readable: "e'" }, // ॆ
    Sym { c: '\u{094a}', kind: Kind::Sign, iso: "o", readable: "o'" }, // ॊ
    Sym { c: '\u{0945}', kind: Kind::Sign, iso: "æ", readable: "a" },  // ॅ
    Sym { c: '\u{0949}', kind: Kind::Sign, iso: "ô", readable: "o" },  // ॉ
    Sym { c: '\u{094d}', kind: Kind::Virama, iso: "×", readable: "×" }, // ्
    Sym { c: '\u{0915}', kind: Kind::Consonant, iso: "k", readable: "k" }, // क
    Sym { c: '\u{0916}', kind: Kind::Consonant, iso: "kh", readable: "kh" }, // ख
    Sym { c: '\u{0917}', kind: Kind::Consonant, iso: "g", readable: "g" }, // ग
    Sym { c: '\u{0918}', kind: Kind::Consonant, iso: "gh", readable: "gh" }, // घ
    Sym { c: '\u{0919}', kind: Kind::Consonant, iso: "ṅ", readable: "ng" }, // ङ
    Sym { c: '\u{091a}', kind: Kind::Consonant, iso: "c", readable: "ch" }, // च
    Sym { c: '\u{091b}', kind: Kind::Consonant, iso: "ch", readable: "chh" }, // छ
    Sym { c: '\u{091c}', kind: Kind::Consonant, iso: "j", readable: "j" }, // ज
    Sym { c: '\u{091d}', kind: Kind::Consonant, iso: "jh", readable: "jh" }, // झ
    Sym { c: '\u{091e}', kind: Kind::Consonant, iso: "ñ", readable: "nj" }, // ञ
    Sym { c: '\u{091f}', kind: Kind::Consonant, iso: "ṭ", readable: "t'" }, // ट
    Sym { c: '\u{0920}', kind: Kind::Consonant, iso: "ṭh", readable: "t'h" }, // ठ
    Sym { c: '\u{0921}', kind: Kind::Consonant, iso: "ḍ", readable: "d'" }, // ड
    Sym { c: '\u{0922}', kind: Kind::Consonant, iso: "ḍh", readable: "d'h" }, // ढ
    Sym { c: '\u{0923}', kind: Kind::Consonant, iso: "ṇ", readable: "n" }, // ण
    Sym { c: '\u{0924}', kind: Kind::Consonant, iso: "t", readable: "t" }, // त
    Sym { c: '\u{0925}', kind: Kind::Consonant, iso: "th", readable: "th" }, // थ
    Sym { c: '\u{0926}', kind: Kind::Consonant, iso: "d", readable: "d" }, // द
    Sym { c: '\u{0927}', kind: Kind::Consonant, iso: "dh", readable: "dh" }, // ध
    Sym { c: '\u{0928}', kind: Kind::Consonant, iso: "n", readable: "n" }, // न
    Sym { c: '\u{092a}', kind: Kind::Consonant, iso: "p", readable: "p" }, // प
    Sym { c: '\u{092b}', kind: Kind::Consonant, iso: "ph", readable: "ph" }, // फ
    Sym { c: '\u{092c}', kind: Kind::Consonant, iso: "b", readable: "b" }, // ब
    Sym { c: '\u{092d}', kind: Kind::Consonant, iso: "bh", readable: "bh" }, // भ
    Sym { c: '\u{092e}', kind: Kind::Consonant, iso: "m", readable: "m" }, // म
    Sym { c: '\u{092f}', kind: Kind::Consonant, iso: "y", readable: "y" }, // य
    Sym { c: '\u{0930}', kind: Kind::Consonant, iso: "r", readable: "r" }, // र
    Sym { c: '\u{0932}', kind: Kind::Consonant, iso: "l", readable: "l" }, // ल
    Sym { c: '\u{0935}', kind: Kind::Consonant, iso: "v", readable: "v" }, // व
    Sym { c: '\u{0936}', kind: Kind::Consonant, iso: "ś", readable: "sh" }, // श
    Sym { c: '\u{0937}', kind: Kind::Consonant, iso: "ṣ", readable: "sh" }, // ष
    Sym { c: '\u{0938}', kind: Kind::Consonant, iso: "s", readable: "s" }, // स
    Sym { c: '\u{0939}', kind: Kind::Consonant, iso: "h", readable: "h" }, // ह
    Sym { c: '\u{0933}', kind: Kind::Consonant, iso: "ḷ", readable: "l'" }, // ळ
    Sym { c: '\u{0934}', kind: Kind::Consonant, iso: "ḻ", readable: "zh" }, // ऴ
    Sym { c: '\u{0931}', kind: Kind::Consonant, iso: "ṟ", readable: "r'" }, // ऱ
    Sym { c: '\u{0929}', kind: Kind::Consonant, iso: "ṉ", readable: "n" }, // ऩ
    Sym { c: '\u{0901}', kind: Kind::Chandrabindu, iso: "m̐", readable: "M" }, // ँ
    Sym { c: '\u{0902}', kind: Kind::Anusvara, iso: "ṁ", readable: "M" }, // ं
    Sym { c: '\u{0903}', kind: Kind::Visarga, iso: "ḥ", readable: "h'" }, // ः
    Sym { c: '\u{0950}', kind: Kind::Om, iso: "ōṁ", readable: "om" },  // ॐ
    Sym { c: '\u{0964}', kind: Kind::Danda, iso: ".", readable: "." }, // ।
    Sym { c: '\u{0965}', kind: Kind::Danda, iso: "..", readable: ".." }, // ॥
    Sym { c: '\u{0966}', kind: Kind::Digit, iso: "0", readable: "0" }, // ०
    Sym { c: '\u{0967}', kind: Kind::Digit, iso: "1", readable: "1" }, // १
    Sym { c: '\u{0968}', kind: Kind::Digit, iso: "2", readable: "2" }, // २
    Sym { c: '\u{0969}', kind: Kind::Digit, iso: "3", readable: "3" }, // ३
    Sym { c: '\u{096a}', kind: Kind::Digit, iso: "4", readable: "4" }, // ४
    Sym { c: '\u{096b}', kind: Kind::Digit, iso: "5", readable: "5" }, // ५
    Sym { c: '\u{096c}', kind: Kind::Digit, iso: "6", readable: "6" }, // ६
    Sym { c: '\u{096d}', kind: Kind::Digit, iso: "7", readable: "7" }, // ७
    Sym { c: '\u{096e}', kind: Kind::Digit, iso: "8", readable: "8" }, // ८
    Sym { c: '\u{096f}', kind: Kind::Digit, iso: "9", readable: "9" }, // ९
];
pub(super) static DEVANAGARI_NUKTA: &[(char, &str, &str)] = &[
    ('\u{0915}', "q", "q"),   // क
    ('\u{0916}', "k͟h", "kh"), // ख
    ('\u{0917}', "ġ", "g"),   // ग
    ('\u{091c}', "z", "z"),   // ज
    ('\u{0921}', "ṛ", "r"),   // ड
    ('\u{0922}', "ṛh", "rh"), // ढ
    ('\u{092b}', "f", "f"),   // फ
    ('\u{092f}', "ẏ", "y"),   // य
];

pub(super) static BENGALI: &[Sym] = &[
    Sym { c: '\u{0985}', kind: Kind::Vowel, iso: "a", readable: "a" }, // অ
    Sym { c: '\u{0986}', kind: Kind::Vowel, iso: "ā", readable: "aa" }, // আ
    Sym { c: '\u{0987}', kind: Kind::Vowel, iso: "i", readable: "i" }, // ই
    Sym { c: '\u{0988}', kind: Kind::Vowel, iso: "ī", readable: "ee" }, // ঈ
    Sym { c: '\u{0989}', kind: Kind::Vowel, iso: "u", readable: "u" }, // উ
    Sym { c: '\u{098a}', kind: Kind::Vowel, iso: "ū", readable: "oo" }, // ঊ
    Sym { c: '\u{098b}', kind: Kind::Vowel, iso: "r̥", readable: "ri'" }, // ঋ
    Sym { c: '\u{09e0}', kind: Kind::Vowel, iso: "r̥̄", readable: "ree'" }, // ৠ
    Sym { c: '\u{098c}', kind: Kind::Vowel, iso: "l̥", readable: "li'" }, // ঌ
    Sym { c: '\u{09e1}', kind: Kind::Vowel, iso: "l̥̄", readable: "lee'" }, // ৡ
    Sym { c: '\u{098f}', kind: Kind::Vowel, iso: "ē", readable: "e" }, // এ
    Sym { c: '\u{0990}', kind: Kind::Vowel, iso: "ai", readable: "ai" }, // ঐ
    Sym { c: '\u{0993}', kind: Kind::Vowel, iso: "ō", readable: "o" }, // ও
    Sym { c: '\u{0994}', kind: Kind::Vowel, iso: "au", readable: "au" }, // ঔ
    Sym { c: '\u{09be}', kind: Kind::Sign, iso: "ā", readable: "aa" }, // া
    Sym { c: '\u{09bf}', kind: Kind::Sign, iso: "i", readable: "i" },  // ি
    Sym { c: '\u{09c0}', kind: Kind::Sign, iso: "ī", readable: "ee" }, // ী
    Sym { c: '\u{09c1}', kind: Kind::Sign, iso: "u", readable: "u" },  // ু
    Sym { c: '\u{09c2}', kind: Kind::Sign, iso: "ū", readable: "oo" }, // ূ
    Sym { c: '\u{09c3}', kind: Kind::Sign, iso: "r̥", readable: "ri'" }, // ৃ
    Sym { c: '\u{09c4}', kind: Kind::Sign, iso: "r̥̄", readable: "ree'" }, // ৄ
    Sym { c: '\u{09e2}', kind: Kind::Sign, iso: "l̥", readable: "li'" }, // ৢ
    Sym { c: '\u{09e3}', kind: Kind::Sign, iso: "l̥̄", readable: "lee'" }, // ৣ
    Sym { c: '\u{09c7}', kind: Kind::Sign, iso: "ē", readable: "e" },  // ে
    Sym { c: '\u{09c8}', kind: Kind::Sign, iso: "ai", readable: "ai" }, // ৈ
    Sym { c: '\u{09cb}', kind: Kind::Sign, iso: "ō", readable: "o" },  // ো
    Sym { c: '\u{09cc}', kind: Kind::Sign, iso: "au", readable: "au" }, // ৌ
    Sym { c: '\u{09cd}', kind: Kind::Virama, iso: "×", readable: "×" }, // ্
    Sym { c: '\u{0995}', kind: Kind::Consonant, iso: "k", readable: "k" }, // ক
    Sym { c: '\u{0996}', kind: Kind::Consonant, iso: "kh", readable: "kh" }, // খ
    Sym { c: '\u{0997}', kind: Kind::Consonant, iso: "g", readable: "g" }, // গ
    Sym { c: '\u{0998}', kind: Kind::Consonant, iso: "gh", readable: "gh" }, // ঘ
    Sym { c: '\u{0999}', kind: Kind::Consonant, iso: "ṅ", readable: "ng" }, // ঙ
    Sym { c: '\u{099a}', kind: Kind::Consonant, iso: "c", readable: "ch" }, // চ
    Sym { c: '\u{099b}', kind: Kind::Consonant, iso: "ch", readable: "chh" }, // ছ
    Sym { c: '\u{099c}', kind: Kind::Consonant, iso: "j", readable: "j" }, // জ
    Sym { c: '\u{099d}', kind: Kind::Consonant, iso: "jh", readable: "jh" }, // ঝ
    Sym { c: '\u{099e}', kind: Kind::Consonant, iso: "ñ", readable: "nj" }, // ঞ
    Sym { c: '\u{099f}', kind: Kind::Consonant, iso: "ṭ", readable: "t'" }, // ট
    Sym { c: '\u{09a0}', kind: Kind::Consonant, iso: "ṭh", readable: "t'h" }, // ঠ
    Sym { c: '\u{09a1}', kind: Kind::Consonant, iso: "ḍ", readable: "d'" }, // ড
    Sym { c: '\u{09a2}', kind: Kind::Consonant, iso: "ḍh", readable: "d'h" }, // ঢ
    Sym { c: '\u{09a3}', kind: Kind::Consonant, iso: "ṇ", readable: "n" }, // ণ
    Sym { c: '\u{09a4}', kind: Kind::Consonant, iso: "t", readable: "t" }, // ত
    Sym { c: '\u{09a5}', kind: Kind::Consonant, iso: "th", readable: "th" }, // থ
    Sym { c: '\u{09a6}', kind: Kind::Consonant, iso: "d", readable: "d" }, // দ
    Sym { c: '\u{09a7}', kind: Kind::Consonant, iso: "dh", readable: "dh" }, // ধ
    Sym { c: '\u{09a8}', kind: Kind::Consonant, iso: "n", readable: "n" }, // ন
    Sym { c: '\u{09aa}', kind: Kind::Consonant, iso: "p", readable: "p" }, // প
    Sym { c: '\u{09ab}', kind: Kind::Consonant, iso: "ph", readable: "ph" }, // ফ
    Sym { c: '\u{09ac}', kind: Kind::Consonant, iso: "b", readable: "b" }, // ব
    Sym { c: '\u{09ad}', kind: Kind::Consonant, iso: "bh", readable: "bh" }, // ভ
    Sym { c: '\u{09ae}', kind: Kind::Consonant, iso: "m", readable: "m" }, // ম
    Sym { c: '\u{09af}', kind: Kind::Consonant, iso: "y", readable: "y" }, // য
    Sym { c: '\u{09b0}', kind: Kind::Consonant, iso: "r", readable: "r" }, // র
    Sym { c: '\u{09b2}', kind: Kind::Consonant, iso: "l", readable: "l" }, // ল
    Sym { c: '\u{09b6}', kind: Kind::Consonant, iso: "ś", readable: "sh" }, // শ
    Sym { c: '\u{09b7}', kind: Kind::Consonant, iso: "ṣ", readable: "sh" }, // ষ
    Sym { c: '\u{09b8}', kind: Kind::Consonant, iso: "s", readable: "s" }, // স
    Sym { c: '\u{09b9}', kind: Kind::Consonant, iso: "h", readable: "h" }, // হ
    Sym { c: '\u{0981}', kind: Kind::Chandrabindu, iso: "m̐", readable: "M" }, // ঁ
    Sym { c: '\u{0982}', kind: Kind::Anusvara, iso: "ṁ", readable: "M" }, // ং
    Sym { c: '\u{0983}', kind: Kind::Visarga, iso: "ḥ", readable: "h'" }, // ঃ
    Sym { c: '\u{0964}', kind: Kind::Danda, iso: ".", readable: "." }, // ।
    Sym { c: '\u{0965}', kind: Kind::Danda, iso: "..", readable: ".." }, // ॥
    Sym { c: '\u{09e6}', kind: Kind::Digit, iso: "0", readable: "0" }, // ০
    Sym { c: '\u{09e7}', kind: Kind::Digit, iso: "1", readable: "1" }, // ১
    Sym { c: '\u{09e8}', kind: Kind::Digit, iso: "2", readable: "2" }, // ২
    Sym { c: '\u{09e9}', kind: Kind::Digit, iso: "3", readable: "3" }, // ৩
    Sym { c: '\u{09ea}', kind: Kind::Digit, iso: "4", readable: "4" }, // ৪
    Sym { c: '\u{09eb}', kind: Kind::Digit, iso: "5", readable: "5" }, // ৫
    Sym { c: '\u{09ec}', kind: Kind::Digit, iso: "6", readable: "6" }, // ৬
    Sym { c: '\u{09ed}', kind: Kind::Digit, iso: "7", readable: "7" }, // ৭
    Sym { c: '\u{09ee}', kind: Kind::Digit, iso: "8", readable: "8" }, // ৮
    Sym { c: '\u{09ef}', kind: Kind::Digit, iso: "9", readable: "9" }, // ৯
];
pub(super) static BENGALI_NUKTA: &[(char, &str, &str)] = &[
    ('\u{09ad}', "v", "v"),   // ভ
    ('\u{09b2}', "ḷ", "l'"),  // ল
    ('\u{09b7}', "ḻ", "zh"),  // ষ
    ('\u{09b0}', "ṟ", "r'"),  // র
    ('\u{09a8}', "ṉ", "n"),   // ন
    ('\u{0995}', "q", "q"),   // ক
    ('\u{0996}', "k͟h", "kh"), // খ
    ('\u{0997}', "ġ", "g"),   // গ
    ('\u{099c}', "z", "z"),   // জ
    ('\u{09a1}', "ṛ", "r"),   // ড
    ('\u{09a2}', "ṛh", "rh"), // ঢ
    ('\u{09ab}', "f", "f"),   // ফ
    ('\u{09af}', "ẏ", "y"),   // য
];

pub(super) static GURMUKHI: &[Sym] = &[
    Sym { c: '\u{0a05}', kind: Kind::Vowel, iso: "a", readable: "a" }, // ਅ
    Sym { c: '\u{0a06}', kind: Kind::Vowel, iso: "ā", readable: "aa" }, // ਆ
    Sym { c: '\u{0a07}', kind: Kind::Vowel, iso: "i", readable: "i" }, // ਇ
    Sym { c: '\u{0a08}', kind: Kind::Vowel, iso: "ī", readable: "ee" }, // ਈ
    Sym { c: '\u{0a09}', kind: Kind::Vowel, iso: "u", readable: "u" }, // ਉ
    Sym { c: '\u{0a0a}', kind: Kind::Vowel, iso: "ū", readable: "oo" }, // ਊ
    Sym { c: '\u{0a0f}', kind: Kind::Vowel, iso: "ē", readable: "e" }, // ਏ
    Sym { c: '\u{0a10}', kind: Kind::Vowel, iso: "ai", readable: "ai" }, // ਐ
    Sym { c: '\u{0a13}', kind: Kind::Vowel, iso: "ō", readable: "o" }, // ਓ
    Sym { c: '\u{0a14}', kind: Kind::Vowel, iso: "au", readable: "au" }, // ਔ
    Sym { c: '\u{0a3e}', kind: Kind::Sign, iso: "ā", readable: "aa" }, // ਾ
    Sym { c: '\u{0a3f}', kind: Kind::Sign, iso: "i", readable: "i" },  // ਿ
    Sym { c: '\u{0a40}', kind: Kind::Sign, iso: "ī", readable: "ee" }, // ੀ
    Sym { c: '\u{0a41}', kind: Kind::Sign, iso: "u", readable: "u" },  // ੁ
    Sym { c: '\u{0a42}', kind: Kind::Sign, iso: "ū", readable: "oo" }, // ੂ
    Sym { c: '\u{0a47}', kind: Kind::Sign, iso: "ē", readable: "e" },  // ੇ
    Sym { c: '\u{0a48}', kind: Kind::Sign, iso: "ai", readable: "ai" }, // ੈ
    Sym { c: '\u{0a4b}', kind: Kind::Sign, iso: "ō", readable: "o" },  // ੋ
    Sym { c: '\u{0a4c}', kind: Kind::Sign, iso: "au", readable: "au" }, // ੌ
    Sym { c: '\u{0a4d}', kind: Kind::Virama, iso: "×", readable: "×" }, // ੍
    Sym { c: '\u{0a15}', kind: Kind::Consonant, iso: "k", readable: "k" }, // ਕ
    Sym { c: '\u{0a16}', kind: Kind::Consonant, iso: "kh", readable: "kh" }, // ਖ
    Sym { c: '\u{0a17}', kind: Kind::Consonant, iso: "g", readable: "g" }, // ਗ
    Sym { c: '\u{0a18}', kind: Kind::Consonant, iso: "gh", readable: "gh" }, // ਘ
    Sym { c: '\u{0a19}', kind: Kind::Consonant, iso: "ṅ", readable: "ng" }, // ਙ
    Sym { c: '\u{0a1a}', kind: Kind::Consonant, iso: "c", readable: "ch" }, // ਚ
    Sym { c: '\u{0a1b}', kind: Kind::Consonant, iso: "ch", readable: "chh" }, // ਛ
    Sym { c: '\u{0a1c}', kind: Kind::Consonant, iso: "j", readable: "j" }, // ਜ
    Sym { c: '\u{0a1d}', kind: Kind::Consonant, iso: "jh", readable: "jh" }, // ਝ
    Sym { c: '\u{0a1e}', kind: Kind::Consonant, iso: "ñ", readable: "nj" }, // ਞ
    Sym { c: '\u{0a1f}', kind: Kind::Consonant, iso: "ṭ", readable: "t'" }, // ਟ
    Sym { c: '\u{0a20}', kind: Kind::Consonant, iso: "ṭh", readable: "t'h" }, // ਠ
    Sym { c: '\u{0a21}', kind: Kind::Consonant, iso: "ḍ", readable: "d'" }, // ਡ
    Sym { c: '\u{0a22}', kind: Kind::Consonant, iso: "ḍh", readable: "d'h" }, // ਢ
    Sym { c: '\u{0a23}', kind: Kind::Consonant, iso: "ṇ", readable: "n" }, // ਣ
    Sym { c: '\u{0a24}', kind: Kind::Consonant, iso: "t", readable: "t" }, // ਤ
    Sym { c: '\u{0a25}', kind: Kind::Consonant, iso: "th", readable: "th" }, // ਥ
    Sym { c: '\u{0a26}', kind: Kind::Consonant, iso: "d", readable: "d" }, // ਦ
    Sym { c: '\u{0a27}', kind: Kind::Consonant, iso: "dh", readable: "dh" }, // ਧ
    Sym { c: '\u{0a28}', kind: Kind::Consonant, iso: "n", readable: "n" }, // ਨ
    Sym { c: '\u{0a2a}', kind: Kind::Consonant, iso: "p", readable: "p" }, // ਪ
    Sym { c: '\u{0a2b}', kind: Kind::Consonant, iso: "ph", readable: "ph" }, // ਫ
    Sym { c: '\u{0a2c}', kind: Kind::Consonant, iso: "b", readable: "b" }, // ਬ
    Sym { c: '\u{0a2d}', kind: Kind::Consonant, iso: "bh", readable: "bh" }, // ਭ
    Sym { c: '\u{0a2e}', kind: Kind::Consonant, iso: "m", readable: "m" }, // ਮ
    Sym { c: '\u{0a2f}', kind: Kind::Consonant, iso: "y", readable: "y" }, // ਯ
    Sym { c: '\u{0a30}', kind: Kind::Consonant, iso: "r", readable: "r" }, // ਰ
    Sym { c: '\u{0a32}', kind: Kind::Consonant, iso: "l", readable: "l" }, // ਲ
    Sym { c: '\u{0a35}', kind: Kind::Consonant, iso: "v", readable: "v" }, // ਵ
    Sym { c: '\u{0a38}', kind: Kind::Consonant, iso: "s", readable: "s" }, // ਸ
    Sym { c: '\u{0a39}', kind: Kind::Consonant, iso: "h", readable: "h" }, // ਹ
    Sym { c: '\u{0a5c}', kind: Kind::Consonant, iso: "ṛ", readable: "r" }, // ੜ
    Sym { c: '\u{0a01}', kind: Kind::Chandrabindu, iso: "m̐", readable: "M" }, // ਁ
    Sym { c: '\u{0a02}', kind: Kind::Anusvara, iso: "ṁ", readable: "M" }, // ਂ
    Sym { c: '\u{0a03}', kind: Kind::Visarga, iso: "ḥ", readable: "h'" }, // ਃ
    Sym { c: '\u{0a74}', kind: Kind::Om, iso: "ōṁ", readable: "om" },  // ੴ
    Sym { c: '\u{0964}', kind: Kind::Danda, iso: ".", readable: "." }, // ।
    Sym { c: '\u{0965}', kind: Kind::Danda, iso: "..", readable: ".." }, // ॥
    Sym { c: '\u{0a66}', kind: Kind::Digit, iso: "0", readable: "0" }, // ੦
    Sym { c: '\u{0a67}', kind: Kind::Digit, iso: "1", readable: "1" }, // ੧
    Sym { c: '\u{0a68}', kind: Kind::Digit, iso: "2", readable: "2" }, // ੨
    Sym { c: '\u{0a69}', kind: Kind::Digit, iso: "3", readable: "3" }, // ੩
    Sym { c: '\u{0a6a}', kind: Kind::Digit, iso: "4", readable: "4" }, // ੪
    Sym { c: '\u{0a6b}', kind: Kind::Digit, iso: "5", readable: "5" }, // ੫
    Sym { c: '\u{0a6c}', kind: Kind::Digit, iso: "6", readable: "6" }, // ੬
    Sym { c: '\u{0a6d}', kind: Kind::Digit, iso: "7", readable: "7" }, // ੭
    Sym { c: '\u{0a6e}', kind: Kind::Digit, iso: "8", readable: "8" }, // ੮
    Sym { c: '\u{0a6f}', kind: Kind::Digit, iso: "9", readable: "9" }, // ੯
];
pub(super) static GURMUKHI_NUKTA: &[(char, &str, &str)] = &[
    ('\u{0a38}', "ś", "sh"),  // ਸ
    ('\u{0a32}', "ḷ", "l'"),  // ਲ
    ('\u{0a30}', "ṟ", "r'"),  // ਰ
    ('\u{0a28}', "ṉ", "n"),   // ਨ
    ('\u{0a15}', "q", "q"),   // ਕ
    ('\u{0a16}', "k͟h", "kh"), // ਖ
    ('\u{0a17}', "ġ", "g"),   // ਗ
    ('\u{0a1c}', "z", "z"),   // ਜ
    ('\u{0a2b}', "f", "f"),   // ਫ
    ('\u{0a2f}', "ẏ", "y"),   // ਯ
];

pub(super) static GUJARATI: &[Sym] = &[
    Sym { c: '\u{0a85}', kind: Kind::Vowel, iso: "a", readable: "a" }, // અ
    Sym { c: '\u{0a86}', kind: Kind::Vowel, iso: "ā", readable: "aa" }, // આ
    Sym { c: '\u{0a87}', kind: Kind::Vowel, iso: "i", readable: "i" }, // ઇ
    Sym { c: '\u{0a88}', kind: Kind::Vowel, iso: "ī", readable: "ee" }, // ઈ
    Sym { c: '\u{0a89}', kind: Kind::Vowel, iso: "u", readable: "u" }, // ઉ
    Sym { c: '\u{0a8a}', kind: Kind::Vowel, iso: "ū", readable: "oo" }, // ઊ
    Sym { c: '\u{0a8b}', kind: Kind::Vowel, iso: "r̥", readable: "ri'" }, // ઋ
    Sym { c: '\u{0ae0}', kind: Kind::Vowel, iso: "r̥̄", readable: "ree'" }, // ૠ
    Sym { c: '\u{0a8c}', kind: Kind::Vowel, iso: "l̥", readable: "li'" }, // ઌ
    Sym { c: '\u{0ae1}', kind: Kind::Vowel, iso: "l̥̄", readable: "lee'" }, // ૡ
    Sym { c: '\u{0a8f}', kind: Kind::Vowel, iso: "ē", readable: "e" }, // એ
    Sym { c: '\u{0a90}', kind: Kind::Vowel, iso: "ai", readable: "ai" }, // ઐ
    Sym { c: '\u{0a93}', kind: Kind::Vowel, iso: "ō", readable: "o" }, // ઓ
    Sym { c: '\u{0a94}', kind: Kind::Vowel, iso: "au", readable: "au" }, // ઔ
    Sym { c: '\u{0a8d}', kind: Kind::Vowel, iso: "æ", readable: "a" }, // ઍ
    Sym { c: '\u{0a91}', kind: Kind::Vowel, iso: "ô", readable: "o" }, // ઑ
    Sym { c: '\u{0abe}', kind: Kind::Sign, iso: "ā", readable: "aa" }, // ા
    Sym { c: '\u{0abf}', kind: Kind::Sign, iso: "i", readable: "i" },  // િ
    Sym { c: '\u{0ac0}', kind: Kind::Sign, iso: "ī", readable: "ee" }, // ી
    Sym { c: '\u{0ac1}', kind: Kind::Sign, iso: "u", readable: "u" },  // ુ
    Sym { c: '\u{0ac2}', kind: Kind::Sign, iso: "ū", readable: "oo" }, // ૂ
    Sym { c: '\u{0ac3}', kind: Kind::Sign, iso: "r̥", readable: "ri'" }, // ૃ
    Sym { c: '\u{0ac4}', kind: Kind::Sign, iso: "r̥̄", readable: "ree'" }, // ૄ
    Sym { c: '\u{0ae2}', kind: Kind::Sign, iso: "l̥", readable: "li'" }, // ૢ
    Sym { c: '\u{0ae3}', kind: Kind::Sign, iso: "l̥̄", readable: "lee'" }, // ૣ
    Sym { c: '\u{0ac7}', kind: Kind::Sign, iso: "ē", readable: "e" },  // ે
    Sym { c: '\u{0ac8}', kind: Kind::Sign, iso: "ai", readable: "ai" }, // ૈ
    Sym { c: '\u{0acb}', kind: Kind::Sign, iso: "ō", readable: "o" },  // ો
    Sym { c: '\u{0acc}', kind: Kind::Sign, iso: "au", readable: "au" }, // ૌ
    Sym { c: '\u{0ac5}', kind: Kind::Sign, iso: "æ", readable: "a" },  // ૅ
    Sym { c: '\u{0ac9}', kind: Kind::Sign, iso: "ô", readable: "o" },  // ૉ
    Sym { c: '\u{0acd}', kind: Kind::Virama, iso: "×", readable: "×" }, // ્
    Sym { c: '\u{0a95}', kind: Kind::Consonant, iso: "k", readable: "k" }, // ક
    Sym { c: '\u{0a96}', kind: Kind::Consonant, iso: "kh", readable: "kh" }, // ખ
    Sym { c: '\u{0a97}', kind: Kind::Consonant, iso: "g", readable: "g" }, // ગ
    Sym { c: '\u{0a98}', kind: Kind::Consonant, iso: "gh", readable: "gh" }, // ઘ
    Sym { c: '\u{0a99}', kind: Kind::Consonant, iso: "ṅ", readable: "ng" }, // ઙ
    Sym { c: '\u{0a9a}', kind: Kind::Consonant, iso: "c", readable: "ch" }, // ચ
    Sym { c: '\u{0a9b}', kind: Kind::Consonant, iso: "ch", readable: "chh" }, // છ
    Sym { c: '\u{0a9c}', kind: Kind::Consonant, iso: "j", readable: "j" }, // જ
    Sym { c: '\u{0a9d}', kind: Kind::Consonant, iso: "jh", readable: "jh" }, // ઝ
    Sym { c: '\u{0a9e}', kind: Kind::Consonant, iso: "ñ", readable: "nj" }, // ઞ
    Sym { c: '\u{0a9f}', kind: Kind::Consonant, iso: "ṭ", readable: "t'" }, // ટ
    Sym { c: '\u{0aa0}', kind: Kind::Consonant, iso: "ṭh", readable: "t'h" }, // ઠ
    Sym { c: '\u{0aa1}', kind: Kind::Consonant, iso: "ḍ", readable: "d'" }, // ડ
    Sym { c: '\u{0aa2}', kind: Kind::Consonant, iso: "ḍh", readable: "d'h" }, // ઢ
    Sym { c: '\u{0aa3}', kind: Kind::Consonant, iso: "ṇ", readable: "n" }, // ણ
    Sym { c: '\u{0aa4}', kind: Kind::Consonant, iso: "t", readable: "t" }, // ત
    Sym { c: '\u{0aa5}', kind: Kind::Consonant, iso: "th", readable: "th" }, // થ
    Sym { c: '\u{0aa6}', kind: Kind::Consonant, iso: "d", readable: "d" }, // દ
    Sym { c: '\u{0aa7}', kind: Kind::Consonant, iso: "dh", readable: "dh" }, // ધ
    Sym { c: '\u{0aa8}', kind: Kind::Consonant, iso: "n", readable: "n" }, // ન
    Sym { c: '\u{0aaa}', kind: Kind::Consonant, iso: "p", readable: "p" }, // પ
    Sym { c: '\u{0aab}', kind: Kind::Consonant, iso: "ph", readable: "ph" }, // ફ
    Sym { c: '\u{0aac}', kind: Kind::Consonant, iso: "b", readable: "b" }, // બ
    Sym { c: '\u{0aad}', kind: Kind::Consonant, iso: "bh", readable: "bh" }, // ભ
    Sym { c: '\u{0aae}', kind: Kind::Consonant, iso: "m", readable: "m" }, // મ
    Sym { c: '\u{0aaf}', kind: Kind::Consonant, iso: "y", readable: "y" }, // ય
    Sym { c: '\u{0ab0}', kind: Kind::Consonant, iso: "r", readable: "r" }, // ર
    Sym { c: '\u{0ab2}', kind: Kind::Consonant, iso: "l", readable: "l" }, // લ
    Sym { c: '\u{0ab5}', kind: Kind::Consonant, iso: "v", readable: "v" }, // વ
    Sym { c: '\u{0ab6}', kind: Kind::Consonant, iso: "ś", readable: "sh" }, // શ
    Sym { c: '\u{0ab7}', kind: Kind::Consonant, iso: "ṣ", readable: "sh" }, // ષ
    Sym { c: '\u{0ab8}', kind: Kind::Consonant, iso: "s", readable: "s" }, // સ
    Sym { c: '\u{0ab9}', kind: Kind::Consonant, iso: "h", readable: "h" }, // હ
    Sym { c: '\u{0ab3}', kind: Kind::Consonant, iso: "ḷ", readable: "l'" }, // ળ
    Sym { c: '\u{0a81}', kind: Kind::Chandrabindu, iso: "m̐", readable: "M" }, // ઁ
    Sym { c: '\u{0a82}', kind: Kind::Anusvara, iso: "ṁ", readable: "M" }, // ં
    Sym { c: '\u{0a83}', kind: Kind::Visarga, iso: "ḥ", readable: "h'" }, // ઃ
    Sym { c: '\u{0ad0}', kind: Kind::Om, iso: "ōṁ", readable: "om" },  // ૐ
    Sym { c: '\u{0964}', kind: Kind::Danda, iso: ".", readable: "." }, // ।
    Sym { c: '\u{0965}', kind: Kind::Danda, iso: "..", readable: ".." }, // ॥
    Sym { c: '\u{0ae6}', kind: Kind::Digit, iso: "0", readable: "0" }, // ૦
    Sym { c: '\u{0ae7}', kind: Kind::Digit, iso: "1", readable: "1" }, // ૧
    Sym { c: '\u{0ae8}', kind: Kind::Digit, iso: "2", readable: "2" }, // ૨
    Sym { c: '\u{0ae9}', kind: Kind::Digit, iso: "3", readable: "3" }, // ૩
    Sym { c: '\u{0aea}', kind: Kind::Digit, iso: "4", readable: "4" }, // ૪
    Sym { c: '\u{0aeb}', kind: Kind::Digit, iso: "5", readable: "5" }, // ૫
    Sym { c: '\u{0aec}', kind: Kind::Digit, iso: "6", readable: "6" }, // ૬
    Sym { c: '\u{0aed}', kind: Kind::Digit, iso: "7", readable: "7" }, // ૭
    Sym { c: '\u{0aee}', kind: Kind::Digit, iso: "8", readable: "8" }, // ૮
    Sym { c: '\u{0aef}', kind: Kind::Digit, iso: "9", readable: "9" }, // ૯
];
pub(super) static GUJARATI_NUKTA: &[(char, &str, &str)] = &[
    ('\u{0ab3}', "ḻ", "zh"),  // ળ
    ('\u{0ab0}', "ṟ", "r'"),  // ર
    ('\u{0aa8}', "ṉ", "n"),   // ન
    ('\u{0a95}', "q", "q"),   // ક
    ('\u{0a96}', "k͟h", "kh"), // ખ
    ('\u{0a97}', "ġ", "g"),   // ગ
    ('\u{0a9c}', "z", "z"),   // જ
    ('\u{0aa1}', "ṛ", "r"),   // ડ
    ('\u{0aa2}', "ṛh", "rh"), // ઢ
    ('\u{0aab}', "f", "f"),   // ફ
    ('\u{0aaf}', "ẏ", "y"),   // ય
];

pub(super) static TAMIL: &[Sym] = &[
    Sym { c: '\u{0b85}', kind: Kind::Vowel, iso: "a", readable: "a" }, // அ
    Sym { c: '\u{0b86}', kind: Kind::Vowel, iso: "ā", readable: "aa" }, // ஆ
    Sym { c: '\u{0b87}', kind: Kind::Vowel, iso: "i", readable: "i" }, // இ
    Sym { c: '\u{0b88}', kind: Kind::Vowel, iso: "ī", readable: "ee" }, // ஈ
    Sym { c: '\u{0b89}', kind: Kind::Vowel, iso: "u", readable: "u" }, // உ
    Sym { c: '\u{0b8a}', kind: Kind::Vowel, iso: "ū", readable: "oo" }, // ஊ
    Sym { c: '\u{0b8f}', kind: Kind::Vowel, iso: "ē", readable: "e" }, // ஏ
    Sym { c: '\u{0b90}', kind: Kind::Vowel, iso: "ai", readable: "ai" }, // ஐ
    Sym { c: '\u{0b93}', kind: Kind::Vowel, iso: "ō", readable: "o" }, // ஓ
    Sym { c: '\u{0b94}', kind: Kind::Vowel, iso: "au", readable: "au" }, // ஔ
    Sym { c: '\u{0b8e}', kind: Kind::Vowel, iso: "e", readable: "e'" }, // எ
    Sym { c: '\u{0b92}', kind: Kind::Vowel, iso: "o", readable: "o'" }, // ஒ
    Sym { c: '\u{0bbe}', kind: Kind::Sign, iso: "ā", readable: "aa" }, // ா
    Sym { c: '\u{0bbf}', kind: Kind::Sign, iso: "i", readable: "i" },  // ி
    Sym { c: '\u{0bc0}', kind: Kind::Sign, iso: "ī", readable: "ee" }, // ீ
    Sym { c: '\u{0bc1}', kind: Kind::Sign, iso: "u", readable: "u" },  // ு
    Sym { c: '\u{0bc2}', kind: Kind::Sign, iso: "ū", readable: "oo" }, // ூ
    Sym { c: '\u{0bc7}', kind: Kind::Sign, iso: "ē", readable: "e" },  // ே
    Sym { c: '\u{0bc8}', kind: Kind::Sign, iso: "ai", readable: "ai" }, // ை
    Sym { c: '\u{0bcb}', kind: Kind::Sign, iso: "ō", readable: "o" },  // ோ
    Sym { c: '\u{0bcc}', kind: Kind::Sign, iso: "au", readable: "au" }, // ௌ
    Sym { c: '\u{0bc6}', kind: Kind::Sign, iso: "e", readable: "e'" }, // ெ
    Sym { c: '\u{0bca}', kind: Kind::Sign, iso: "o", readable: "o'" }, // ொ
    Sym { c: '\u{0bcd}', kind: Kind::Virama, iso: "×", readable: "×" }, // ்
    Sym { c: '\u{0b95}', kind: Kind::Consonant, iso: "k", readable: "k" }, // க
    Sym { c: '\u{0b99}', kind: Kind::Consonant, iso: "ṅ", readable: "ng" }, // ங
    Sym { c: '\u{0b9a}', kind: Kind::Consonant, iso: "c", readable: "ch" }, // ச
    Sym { c: '\u{0b9c}', kind: Kind::Consonant, iso: "j", readable: "j" }, // ஜ
    Sym { c: '\u{0b9e}', kind: Kind::Consonant, iso: "ñ", readable: "nj" }, // ஞ
    Sym { c: '\u{0b9f}', kind: Kind::Consonant, iso: "ṭ", readable: "t'" }, // ட
    Sym { c: '\u{0ba3}', kind: Kind::Consonant, iso: "ṇ", readable: "n" }, // ண
    Sym { c: '\u{0ba4}', kind: Kind::Consonant, iso: "t", readable: "t" }, // த
    Sym { c: '\u{0ba8}', kind: Kind::Consonant, iso: "n", readable: "n" }, // ந
    Sym { c: '\u{0baa}', kind: Kind::Consonant, iso: "p", readable: "p" }, // ப
    Sym { c: '\u{0bae}', kind: Kind::Consonant, iso: "m", readable: "m" }, // ம
    Sym { c: '\u{0baf}', kind: Kind::Consonant, iso: "y", readable: "y" }, // ய
    Sym { c: '\u{0bb0}', kind: Kind::Consonant, iso: "r", readable: "r" }, // ர
    Sym { c: '\u{0bb2}', kind: Kind::Consonant, iso: "l", readable: "l" }, // ல
    Sym { c: '\u{0bb5}', kind: Kind::Consonant, iso: "v", readable: "v" }, // வ
    Sym { c: '\u{0bb6}', kind: Kind::Consonant, iso: "ś", readable: "sh" }, // ஶ
    Sym { c: '\u{0bb7}', kind: Kind::Consonant, iso: "ṣ", readable: "sh" }, // ஷ
    Sym { c: '\u{0bb8}', kind: Kind::Consonant, iso: "s", readable: "s" }, // ஸ
    Sym { c: '\u{0bb9}', kind: Kind::Consonant, iso: "h", readable: "h" }, // ஹ
    Sym { c: '\u{0bb3}', kind: Kind::Consonant, iso: "ḷ", readable: "l'" }, // ள
    Sym { c: '\u{0bb4}', kind: Kind::Consonant, iso: "ḻ", readable: "zh" }, // ழ
    Sym { c: '\u{0bb1}', kind: Kind::Consonant, iso: "ṟ", readable: "r'" }, // ற
    Sym { c: '\u{0ba9}', kind: Kind::Consonant, iso: "ṉ", readable: "n" }, // ன
    Sym { c: '\u{a789}', kind: Kind::Visarga, iso: "ḥ", readable: "h'" }, // ꞉
    Sym { c: '\u{0b83}', kind: Kind::Aytham, iso: "ḵ", readable: "g" }, // ஃ
    Sym { c: '\u{0bd0}', kind: Kind::Om, iso: "ōṁ", readable: "om" },  // ௐ
    Sym { c: '\u{0964}', kind: Kind::Danda, iso: ".", readable: "." }, // ।
    Sym { c: '\u{0965}', kind: Kind::Danda, iso: "..", readable: ".." }, // ॥
    Sym { c: '\u{0be6}', kind: Kind::Digit, iso: "0", readable: "0" }, // ௦
    Sym { c: '\u{0be7}', kind: Kind::Digit, iso: "1", readable: "1" }, // ௧
    Sym { c: '\u{0be8}', kind: Kind::Digit, iso: "2", readable: "2" }, // ௨
    Sym { c: '\u{0be9}', kind: Kind::Digit, iso: "3", readable: "3" }, // ௩
    Sym { c: '\u{0bea}', kind: Kind::Digit, iso: "4", readable: "4" }, // ௪
    Sym { c: '\u{0beb}', kind: Kind::Digit, iso: "5", readable: "5" }, // ௫
    Sym { c: '\u{0bec}', kind: Kind::Digit, iso: "6", readable: "6" }, // ௬
    Sym { c: '\u{0bed}', kind: Kind::Digit, iso: "7", readable: "7" }, // ௭
    Sym { c: '\u{0bee}', kind: Kind::Digit, iso: "8", readable: "8" }, // ௮
    Sym { c: '\u{0bef}', kind: Kind::Digit, iso: "9", readable: "9" }, // ௯
];
pub(super) static TAMIL_NUKTA: &[(char, &str, &str)] = &[];

pub(super) static TELUGU: &[Sym] = &[
    Sym { c: '\u{0c05}', kind: Kind::Vowel, iso: "a", readable: "a" }, // అ
    Sym { c: '\u{0c06}', kind: Kind::Vowel, iso: "ā", readable: "aa" }, // ఆ
    Sym { c: '\u{0c07}', kind: Kind::Vowel, iso: "i", readable: "i" }, // ఇ
    Sym { c: '\u{0c08}', kind: Kind::Vowel, iso: "ī", readable: "ee" }, // ఈ
    Sym { c: '\u{0c09}', kind: Kind::Vowel, iso: "u", readable: "u" }, // ఉ
    Sym { c: '\u{0c0a}', kind: Kind::Vowel, iso: "ū", readable: "oo" }, // ఊ
    Sym { c: '\u{0c0b}', kind: Kind::Vowel, iso: "r̥", readable: "ri'" }, // ఋ
    Sym { c: '\u{0c60}', kind: Kind::Vowel, iso: "r̥̄", readable: "ree'" }, // ౠ
    Sym { c: '\u{0c0c}', kind: Kind::Vowel, iso: "l̥", readable: "li'" }, // ఌ
    Sym { c: '\u{0c61}', kind: Kind::Vowel, iso: "l̥̄", readable: "lee'" }, // ౡ
    Sym { c: '\u{0c0f}', kind: Kind::Vowel, iso: "ē", readable: "e" }, // ఏ
    Sym { c: '\u{0c10}', kind: Kind::Vowel, iso: "ai", readable: "ai" }, // ఐ
    Sym { c: '\u{0c13}', kind: Kind::Vowel, iso: "ō", readable: "o" }, // ఓ
    Sym { c: '\u{0c14}', kind: Kind::Vowel, iso: "au", readable: "au" }, // ఔ
    Sym { c: '\u{0c0e}', kind: Kind::Vowel, iso: "e", readable: "e'" }, // ఎ
    Sym { c: '\u{0c12}', kind: Kind::Vowel, iso: "o", readable: "o'" }, // ఒ
    Sym { c: '\u{0c3e}', kind: Kind::Sign, iso: "ā", readable: "aa" }, // ా
    Sym { c: '\u{0c3f}', kind: Kind::Sign, iso: "i", readable: "i" },  // ి
    Sym { c: '\u{0c40}', kind: Kind::Sign, iso: "ī", readable: "ee" }, // ీ
    Sym { c: '\u{0c41}', kind: Kind::Sign, iso: "u", readable: "u" },  // ు
    Sym { c: '\u{0c42}', kind: Kind::Sign, iso: "ū", readable: "oo" }, // ూ
    Sym { c: '\u{0c43}', kind: Kind::Sign, iso: "r̥", readable: "ri'" }, // ృ
    Sym { c: '\u{0c44}', kind: Kind::Sign, iso: "r̥̄", readable: "ree'" }, // ౄ
    Sym { c: '\u{0c62}', kind: Kind::Sign, iso: "l̥", readable: "li'" }, // ౢ
    Sym { c: '\u{0c63}', kind: Kind::Sign, iso: "l̥̄", readable: "lee'" }, // ౣ
    Sym { c: '\u{0c47}', kind: Kind::Sign, iso: "ē", readable: "e" },  // ే
    Sym { c: '\u{0c48}', kind: Kind::Sign, iso: "ai", readable: "ai" }, // ై
    Sym { c: '\u{0c4b}', kind: Kind::Sign, iso: "ō", readable: "o" },  // ో
    Sym { c: '\u{0c4c}', kind: Kind::Sign, iso: "au", readable: "au" }, // ౌ
    Sym { c: '\u{0c46}', kind: Kind::Sign, iso: "e", readable: "e'" }, // ె
    Sym { c: '\u{0c4a}', kind: Kind::Sign, iso: "o", readable: "o'" }, // ొ
    Sym { c: '\u{0c4d}', kind: Kind::Virama, iso: "×", readable: "×" }, // ్
    Sym { c: '\u{0c15}', kind: Kind::Consonant, iso: "k", readable: "k" }, // క
    Sym { c: '\u{0c16}', kind: Kind::Consonant, iso: "kh", readable: "kh" }, // ఖ
    Sym { c: '\u{0c17}', kind: Kind::Consonant, iso: "g", readable: "g" }, // గ
    Sym { c: '\u{0c18}', kind: Kind::Consonant, iso: "gh", readable: "gh" }, // ఘ
    Sym { c: '\u{0c19}', kind: Kind::Consonant, iso: "ṅ", readable: "ng" }, // ఙ
    Sym { c: '\u{0c1a}', kind: Kind::Consonant, iso: "c", readable: "ch" }, // చ
    Sym { c: '\u{0c1b}', kind: Kind::Consonant, iso: "ch", readable: "chh" }, // ఛ
    Sym { c: '\u{0c1c}', kind: Kind::Consonant, iso: "j", readable: "j" }, // జ
    Sym { c: '\u{0c1d}', kind: Kind::Consonant, iso: "jh", readable: "jh" }, // ఝ
    Sym { c: '\u{0c1e}', kind: Kind::Consonant, iso: "ñ", readable: "nj" }, // ఞ
    Sym { c: '\u{0c1f}', kind: Kind::Consonant, iso: "ṭ", readable: "t'" }, // ట
    Sym { c: '\u{0c20}', kind: Kind::Consonant, iso: "ṭh", readable: "t'h" }, // ఠ
    Sym { c: '\u{0c21}', kind: Kind::Consonant, iso: "ḍ", readable: "d'" }, // డ
    Sym { c: '\u{0c22}', kind: Kind::Consonant, iso: "ḍh", readable: "d'h" }, // ఢ
    Sym { c: '\u{0c23}', kind: Kind::Consonant, iso: "ṇ", readable: "n" }, // ణ
    Sym { c: '\u{0c24}', kind: Kind::Consonant, iso: "t", readable: "t" }, // త
    Sym { c: '\u{0c25}', kind: Kind::Consonant, iso: "th", readable: "th" }, // థ
    Sym { c: '\u{0c26}', kind: Kind::Consonant, iso: "d", readable: "d" }, // ద
    Sym { c: '\u{0c27}', kind: Kind::Consonant, iso: "dh", readable: "dh" }, // ధ
    Sym { c: '\u{0c28}', kind: Kind::Consonant, iso: "n", readable: "n" }, // న
    Sym { c: '\u{0c2a}', kind: Kind::Consonant, iso: "p", readable: "p" }, // ప
    Sym { c: '\u{0c2b}', kind: Kind::Consonant, iso: "ph", readable: "ph" }, // ఫ
    Sym { c: '\u{0c2c}', kind: Kind::Consonant, iso: "b", readable: "b" }, // బ
    Sym { c: '\u{0c2d}', kind: Kind::Consonant, iso: "bh", readable: "bh" }, // భ
    Sym { c: '\u{0c2e}', kind: Kind::Consonant, iso: "m", readable: "m" }, // మ
    Sym { c: '\u{0c2f}', kind: Kind::Consonant, iso: "y", readable: "y" }, // య
    Sym { c: '\u{0c30}', kind: Kind::Consonant, iso: "r", readable: "r" }, // ర
    Sym { c: '\u{0c32}', kind: Kind::Consonant, iso: "l", readable: "l" }, // ల
    Sym { c: '\u{0c35}', kind: Kind::Consonant, iso: "v", readable: "v" }, // వ
    Sym { c: '\u{0c36}', kind: Kind::Consonant, iso: "ś", readable: "sh" }, // శ
    Sym { c: '\u{0c37}', kind: Kind::Consonant, iso: "ṣ", readable: "sh" }, // ష
    Sym { c: '\u{0c38}', kind: Kind::Consonant, iso: "s", readable: "s" }, // స
    Sym { c: '\u{0c39}', kind: Kind::Consonant, iso: "h", readable: "h" }, // హ
    Sym { c: '\u{0c33}', kind: Kind::Consonant, iso: "ḷ", readable: "l'" }, // ళ
    Sym { c: '\u{0c34}', kind: Kind::Consonant, iso: "ḻ", readable: "zh" }, // ఴ
    Sym { c: '\u{0c31}', kind: Kind::Consonant, iso: "ṟ", readable: "r'" }, // ఱ
    Sym { c: '\u{0c00}', kind: Kind::Chandrabindu, iso: "m̐", readable: "M" }, // ఀ
    Sym { c: '\u{0c02}', kind: Kind::Anusvara, iso: "ṁ", readable: "M" }, // ం
    Sym { c: '\u{0c03}', kind: Kind::Visarga, iso: "ḥ", readable: "h'" }, // ః
    Sym { c: '\u{0964}', kind: Kind::Danda, iso: ".", readable: "." }, // ।
    Sym { c: '\u{0965}', kind: Kind::Danda, iso: "..", readable: ".." }, // ॥
    Sym { c: '\u{0c66}', kind: Kind::Digit, iso: "0", readable: "0" }, // ౦
    Sym { c: '\u{0c67}', kind: Kind::Digit, iso: "1", readable: "1" }, // ౧
    Sym { c: '\u{0c68}', kind: Kind::Digit, iso: "2", readable: "2" }, // ౨
    Sym { c: '\u{0c69}', kind: Kind::Digit, iso: "3", readable: "3" }, // ౩
    Sym { c: '\u{0c6a}', kind: Kind::Digit, iso: "4", readable: "4" }, // ౪
    Sym { c: '\u{0c6b}', kind: Kind::Digit, iso: "5", readable: "5" }, // ౫
    Sym { c: '\u{0c6c}', kind: Kind::Digit, iso: "6", readable: "6" }, // ౬
    Sym { c: '\u{0c6d}', kind: Kind::Digit, iso: "7", readable: "7" }, // ౭
    Sym { c: '\u{0c6e}', kind: Kind::Digit, iso: "8", readable: "8" }, // ౮
    Sym { c: '\u{0c6f}', kind: Kind::Digit, iso: "9", readable: "9" }, // ౯
];
pub(super) static TELUGU_NUKTA: &[(char, &str, &str)] = &[
    ('\u{0c28}', "ṉ", "n"),   // న
    ('\u{0c15}', "q", "q"),   // క
    ('\u{0c16}', "k͟h", "kh"), // ఖ
    ('\u{0c17}', "ġ", "g"),   // గ
    ('\u{0c1c}', "z", "z"),   // జ
    ('\u{0c21}', "ṛ", "r"),   // డ
    ('\u{0c22}', "ṛh", "rh"), // ఢ
    ('\u{0c2b}', "f", "f"),   // ఫ
    ('\u{0c2f}', "ẏ", "y"),   // య
];

pub(super) static KANNADA: &[Sym] = &[
    Sym { c: '\u{0c85}', kind: Kind::Vowel, iso: "a", readable: "a" }, // ಅ
    Sym { c: '\u{0c86}', kind: Kind::Vowel, iso: "ā", readable: "aa" }, // ಆ
    Sym { c: '\u{0c87}', kind: Kind::Vowel, iso: "i", readable: "i" }, // ಇ
    Sym { c: '\u{0c88}', kind: Kind::Vowel, iso: "ī", readable: "ee" }, // ಈ
    Sym { c: '\u{0c89}', kind: Kind::Vowel, iso: "u", readable: "u" }, // ಉ
    Sym { c: '\u{0c8a}', kind: Kind::Vowel, iso: "ū", readable: "oo" }, // ಊ
    Sym { c: '\u{0c8b}', kind: Kind::Vowel, iso: "r̥", readable: "ri'" }, // ಋ
    Sym { c: '\u{0ce0}', kind: Kind::Vowel, iso: "r̥̄", readable: "ree'" }, // ೠ
    Sym { c: '\u{0c8c}', kind: Kind::Vowel, iso: "l̥", readable: "li'" }, // ಌ
    Sym { c: '\u{0ce1}', kind: Kind::Vowel, iso: "l̥̄", readable: "lee'" }, // ೡ
    Sym { c: '\u{0c8f}', kind: Kind::Vowel, iso: "ē", readable: "e" }, // ಏ
    Sym { c: '\u{0c90}', kind: Kind::Vowel, iso: "ai", readable: "ai" }, // ಐ
    Sym { c: '\u{0c93}', kind: Kind::Vowel, iso: "ō", readable: "o" }, // ಓ
    Sym { c: '\u{0c94}', kind: Kind::Vowel, iso: "au", readable: "au" }, // ಔ
    Sym { c: '\u{0c8e}', kind: Kind::Vowel, iso: "e", readable: "e'" }, // ಎ
    Sym { c: '\u{0c92}', kind: Kind::Vowel, iso: "o", readable: "o'" }, // ಒ
    Sym { c: '\u{0cbe}', kind: Kind::Sign, iso: "ā", readable: "aa" }, // ಾ
    Sym { c: '\u{0cbf}', kind: Kind::Sign, iso: "i", readable: "i" },  // ಿ
    Sym { c: '\u{0cc0}', kind: Kind::Sign, iso: "ī", readable: "ee" }, // ೀ
    Sym { c: '\u{0cc1}', kind: Kind::Sign, iso: "u", readable: "u" },  // ು
    Sym { c: '\u{0cc2}', kind: Kind::Sign, iso: "ū", readable: "oo" }, // ೂ
    Sym { c: '\u{0cc3}', kind: Kind::Sign, iso: "r̥", readable: "ri'" }, // ೃ
    Sym { c: '\u{0cc4}', kind: Kind::Sign, iso: "r̥̄", readable: "ree'" }, // ೄ
    Sym { c: '\u{0ce2}', kind: Kind::Sign, iso: "l̥", readable: "li'" }, // ೢ
    Sym { c: '\u{0ce3}', kind: Kind::Sign, iso: "l̥̄", readable: "lee'" }, // ೣ
    Sym { c: '\u{0cc7}', kind: Kind::Sign, iso: "ē", readable: "e" },  // ೇ
    Sym { c: '\u{0cc8}', kind: Kind::Sign, iso: "ai", readable: "ai" }, // ೈ
    Sym { c: '\u{0ccb}', kind: Kind::Sign, iso: "ō", readable: "o" },  // ೋ
    Sym { c: '\u{0ccc}', kind: Kind::Sign, iso: "au", readable: "au" }, // ೌ
    Sym { c: '\u{0cc6}', kind: Kind::Sign, iso: "e", readable: "e'" }, // ೆ
    Sym { c: '\u{0cca}', kind: Kind::Sign, iso: "o", readable: "o'" }, // ೊ
    Sym { c: '\u{0ccd}', kind: Kind::Virama, iso: "×", readable: "×" }, // ್
    Sym { c: '\u{0c95}', kind: Kind::Consonant, iso: "k", readable: "k" }, // ಕ
    Sym { c: '\u{0c96}', kind: Kind::Consonant, iso: "kh", readable: "kh" }, // ಖ
    Sym { c: '\u{0c97}', kind: Kind::Consonant, iso: "g", readable: "g" }, // ಗ
    Sym { c: '\u{0c98}', kind: Kind::Consonant, iso: "gh", readable: "gh" }, // ಘ
    Sym { c: '\u{0c99}', kind: Kind::Consonant, iso: "ṅ", readable: "ng" }, // ಙ
    Sym { c: '\u{0c9a}', kind: Kind::Consonant, iso: "c", readable: "ch" }, // ಚ
    Sym { c: '\u{0c9b}', kind: Kind::Consonant, iso: "ch", readable: "chh" }, // ಛ
    Sym { c: '\u{0c9c}', kind: Kind::Consonant, iso: "j", readable: "j" }, // ಜ
    Sym { c: '\u{0c9d}', kind: Kind::Consonant, iso: "jh", readable: "jh" }, // ಝ
    Sym { c: '\u{0c9e}', kind: Kind::Consonant, iso: "ñ", readable: "nj" }, // ಞ
    Sym { c: '\u{0c9f}', kind: Kind::Consonant, iso: "ṭ", readable: "t'" }, // ಟ
    Sym { c: '\u{0ca0}', kind: Kind::Consonant, iso: "ṭh", readable: "t'h" }, // ಠ
    Sym { c: '\u{0ca1}', kind: Kind::Consonant, iso: "ḍ", readable: "d'" }, // ಡ
    Sym { c: '\u{0ca2}', kind: Kind::Consonant, iso: "ḍh", readable: "d'h" }, // ಢ
    Sym { c: '\u{0ca3}', kind: Kind::Consonant, iso: "ṇ", readable: "n" }, // ಣ
    Sym { c: '\u{0ca4}', kind: Kind::Consonant, iso: "t", readable: "t" }, // ತ
    Sym { c: '\u{0ca5}', kind: Kind::Consonant, iso: "th", readable: "th" }, // ಥ
    Sym { c: '\u{0ca6}', kind: Kind::Consonant, iso: "d", readable: "d" }, // ದ
    Sym { c: '\u{0ca7}', kind: Kind::Consonant, iso: "dh", readable: "dh" }, // ಧ
    Sym { c: '\u{0ca8}', kind: Kind::Consonant, iso: "n", readable: "n" }, // ನ
    Sym { c: '\u{0caa}', kind: Kind::Consonant, iso: "p", readable: "p" }, // ಪ
    Sym { c: '\u{0cab}', kind: Kind::Consonant, iso: "ph", readable: "ph" }, // ಫ
    Sym { c: '\u{0cac}', kind: Kind::Consonant, iso: "b", readable: "b" }, // ಬ
    Sym { c: '\u{0cad}', kind: Kind::Consonant, iso: "bh", readable: "bh" }, // ಭ
    Sym { c: '\u{0cae}', kind: Kind::Consonant, iso: "m", readable: "m" }, // ಮ
    Sym { c: '\u{0caf}', kind: Kind::Consonant, iso: "y", readable: "y" }, // ಯ
    Sym { c: '\u{0cb0}', kind: Kind::Consonant, iso: "r", readable: "r" }, // ರ
    Sym { c: '\u{0cb2}', kind: Kind::Consonant, iso: "l", readable: "l" }, // ಲ
    Sym { c: '\u{0cb5}', kind: Kind::Consonant, iso: "v", readable: "v" }, // ವ
    Sym { c: '\u{0cb6}', kind: Kind::Consonant, iso: "ś", readable: "sh" }, // ಶ
    Sym { c: '\u{0cb7}', kind: Kind::Consonant, iso: "ṣ", readable: "sh" }, // ಷ
    Sym { c: '\u{0cb8}', kind: Kind::Consonant, iso: "s", readable: "s" }, // ಸ
    Sym { c: '\u{0cb9}', kind: Kind::Consonant, iso: "h", readable: "h" }, // ಹ
    Sym { c: '\u{0cb3}', kind: Kind::Consonant, iso: "ḷ", readable: "l'" }, // ಳ
    Sym { c: '\u{0cde}', kind: Kind::Consonant, iso: "ḻ", readable: "zh" }, // ೞ
    Sym { c: '\u{0cb1}', kind: Kind::Consonant, iso: "ṟ", readable: "r'" }, // ಱ
    Sym { c: '\u{0c81}', kind: Kind::Chandrabindu, iso: "m̐", readable: "M" }, // ಁ
    Sym { c: '\u{0c82}', kind: Kind::Anusvara, iso: "ṁ", readable: "M" }, // ಂ
    Sym { c: '\u{0c83}', kind: Kind::Visarga, iso: "ḥ", readable: "h'" }, // ಃ
    Sym { c: '\u{0964}', kind: Kind::Danda, iso: ".", readable: "." }, // ।
    Sym { c: '\u{0965}', kind: Kind::Danda, iso: "..", readable: ".." }, // ॥
    Sym { c: '\u{0ce6}', kind: Kind::Digit, iso: "0", readable: "0" }, // ೦
    Sym { c: '\u{0ce7}', kind: Kind::Digit, iso: "1", readable: "1" }, // ೧
    Sym { c: '\u{0ce8}', kind: Kind::Digit, iso: "2", readable: "2" }, // ೨
    Sym { c: '\u{0ce9}', kind: Kind::Digit, iso: "3", readable: "3" }, // ೩
    Sym { c: '\u{0cea}', kind: Kind::Digit, iso: "4", readable: "4" }, // ೪
    Sym { c: '\u{0ceb}', kind: Kind::Digit, iso: "5", readable: "5" }, // ೫
    Sym { c: '\u{0cec}', kind: Kind::Digit, iso: "6", readable: "6" }, // ೬
    Sym { c: '\u{0ced}', kind: Kind::Digit, iso: "7", readable: "7" }, // ೭
    Sym { c: '\u{0cee}', kind: Kind::Digit, iso: "8", readable: "8" }, // ೮
    Sym { c: '\u{0cef}', kind: Kind::Digit, iso: "9", readable: "9" }, // ೯
];
pub(super) static KANNADA_NUKTA: &[(char, &str, &str)] = &[
    ('\u{0ca8}', "ṉ", "n"),   // ನ
    ('\u{0c95}', "q", "q"),   // ಕ
    ('\u{0c96}', "k͟h", "kh"), // ಖ
    ('\u{0c97}', "ġ", "g"),   // ಗ
    ('\u{0c9c}', "z", "z"),   // ಜ
    ('\u{0ca1}', "ṛ", "r"),   // ಡ
    ('\u{0ca2}', "ṛh", "rh"), // ಢ
    ('\u{0cab}', "f", "f"),   // ಫ
    ('\u{0caf}', "ẏ", "y"),   // ಯ
];

pub(super) static MALAYALAM: &[Sym] = &[
    Sym { c: '\u{0d05}', kind: Kind::Vowel, iso: "a", readable: "a" }, // അ
    Sym { c: '\u{0d06}', kind: Kind::Vowel, iso: "ā", readable: "aa" }, // ആ
    Sym { c: '\u{0d07}', kind: Kind::Vowel, iso: "i", readable: "i" }, // ഇ
    Sym { c: '\u{0d08}', kind: Kind::Vowel, iso: "ī", readable: "ee" }, // ഈ
    Sym { c: '\u{0d09}', kind: Kind::Vowel, iso: "u", readable: "u" }, // ഉ
    Sym { c: '\u{0d0a}', kind: Kind::Vowel, iso: "ū", readable: "oo" }, // ഊ
    Sym { c: '\u{0d0b}', kind: Kind::Vowel, iso: "r̥", readable: "ri'" }, // ഋ
    Sym { c: '\u{0d60}', kind: Kind::Vowel, iso: "r̥̄", readable: "ree'" }, // ൠ
    Sym { c: '\u{0d0c}', kind: Kind::Vowel, iso: "l̥", readable: "li'" }, // ഌ
    Sym { c: '\u{0d61}', kind: Kind::Vowel, iso: "l̥̄", readable: "lee'" }, // ൡ
    Sym { c: '\u{0d0f}', kind: Kind::Vowel, iso: "ē", readable: "e" }, // ഏ
    Sym { c: '\u{0d10}', kind: Kind::Vowel, iso: "ai", readable: "ai" }, // ഐ
    Sym { c: '\u{0d13}', kind: Kind::Vowel, iso: "ō", readable: "o" }, // ഓ
    Sym { c: '\u{0d14}', kind: Kind::Vowel, iso: "au", readable: "au" }, // ഔ
    Sym { c: '\u{0d0e}', kind: Kind::Vowel, iso: "e", readable: "e'" }, // എ
    Sym { c: '\u{0d12}', kind: Kind::Vowel, iso: "o", readable: "o'" }, // ഒ
    Sym { c: '\u{0d3e}', kind: Kind::Sign, iso: "ā", readable: "aa" }, // ാ
    Sym { c: '\u{0d3f}', kind: Kind::Sign, iso: "i", readable: "i" },  // ി
    Sym { c: '\u{0d40}', kind: Kind::Sign, iso: "ī", readable: "ee" }, // ീ
    Sym { c: '\u{0d41}', kind: Kind::Sign, iso: "u", readable: "u" },  // ു
    Sym { c: '\u{0d42}', kind: Kind::Sign, iso: "ū", readable: "oo" }, // ൂ
    Sym { c: '\u{0d43}', kind: Kind::Sign, iso: "r̥", readable: "ri'" }, // ൃ
    Sym { c: '\u{0d44}', kind: Kind::Sign, iso: "r̥̄", readable: "ree'" }, // ൄ
    Sym { c: '\u{0d62}', kind: Kind::Sign, iso: "l̥", readable: "li'" }, // ൢ
    Sym { c: '\u{0d63}', kind: Kind::Sign, iso: "l̥̄", readable: "lee'" }, // ൣ
    Sym { c: '\u{0d47}', kind: Kind::Sign, iso: "ē", readable: "e" },  // േ
    Sym { c: '\u{0d48}', kind: Kind::Sign, iso: "ai", readable: "ai" }, // ൈ
    Sym { c: '\u{0d4b}', kind: Kind::Sign, iso: "ō", readable: "o" },  // ോ
    Sym { c: '\u{0d57}', kind: Kind::Sign, iso: "au", readable: "au" }, // ൗ
    Sym { c: '\u{0d46}', kind: Kind::Sign, iso: "e", readable: "e'" }, // െ
    Sym { c: '\u{0d4a}', kind: Kind::Sign, iso: "o", readable: "o'" }, // ൊ
    Sym { c: '\u{0d4d}', kind: Kind::Virama, iso: "×", readable: "×" }, // ്
    Sym { c: '\u{0d15}', kind: Kind::Consonant, iso: "k", readable: "k" }, // ക
    Sym { c: '\u{0d16}', kind: Kind::Consonant, iso: "kh", readable: "kh" }, // ഖ
    Sym { c: '\u{0d17}', kind: Kind::Consonant, iso: "g", readable: "g" }, // ഗ
    Sym { c: '\u{0d18}', kind: Kind::Consonant, iso: "gh", readable: "gh" }, // ഘ
    Sym { c: '\u{0d19}', kind: Kind::Consonant, iso: "ṅ", readable: "ng" }, // ങ
    Sym { c: '\u{0d1a}', kind: Kind::Consonant, iso: "c", readable: "ch" }, // ച
    Sym { c: '\u{0d1b}', kind: Kind::Consonant, iso: "ch", readable: "chh" }, // ഛ
    Sym { c: '\u{0d1c}', kind: Kind::Consonant, iso: "j", readable: "j" }, // ജ
    Sym { c: '\u{0d1d}', kind: Kind::Consonant, iso: "jh", readable: "jh" }, // ഝ
    Sym { c: '\u{0d1e}', kind: Kind::Consonant, iso: "ñ", readable: "nj" }, // ഞ
    Sym { c: '\u{0d1f}', kind: Kind::Consonant, iso: "ṭ", readable: "t'" }, // ട
    Sym { c: '\u{0d20}', kind: Kind::Consonant, iso: "ṭh", readable: "t'h" }, // ഠ
    Sym { c: '\u{0d21}', kind: Kind::Consonant, iso: "ḍ", readable: "d'" }, // ഡ
    Sym { c: '\u{0d22}', kind: Kind::Consonant, iso: "ḍh", readable: "d'h" }, // ഢ
    Sym { c: '\u{0d23}', kind: Kind::Consonant, iso: "ṇ", readable: "n" }, // ണ
    Sym { c: '\u{0d24}', kind: Kind::Consonant, iso: "t", readable: "t" }, // ത
    Sym { c: '\u{0d25}', kind: Kind::Consonant, iso: "th", readable: "th" }, // ഥ
    Sym { c: '\u{0d26}', kind: Kind::Consonant, iso: "d", readable: "d" }, // ദ
    Sym { c: '\u{0d27}', kind: Kind::Consonant, iso: "dh", readable: "dh" }, // ധ
    Sym { c: '\u{0d28}', kind: Kind::Consonant, iso: "n", readable: "n" }, // ന
    Sym { c: '\u{0d2a}', kind: Kind::Consonant, iso: "p", readable: "p" }, // പ
    Sym { c: '\u{0d2b}', kind: Kind::Consonant, iso: "ph", readable: "ph" }, // ഫ
    Sym { c: '\u{0d2c}', kind: Kind::Consonant, iso: "b", readable: "b" }, // ബ
    Sym { c: '\u{0d2d}', kind: Kind::Consonant, iso: "bh", readable: "bh" }, // ഭ
    Sym { c: '\u{0d2e}', kind: Kind::Consonant, iso: "m", readable: "m" }, // മ
    Sym { c: '\u{0d2f}', kind: Kind::Consonant, iso: "y", readable: "y" }, // യ
    Sym { c: '\u{0d30}', kind: Kind::Consonant, iso: "r", readable: "r" }, // ര
    Sym { c: '\u{0d32}', kind: Kind::Consonant, iso: "l", readable: "l" }, // ല
    Sym { c: '\u{0d35}', kind: Kind::Consonant, iso: "v", readable: "v" }, // വ
    Sym { c: '\u{0d36}', kind: Kind::Consonant, iso: "ś", readable: "sh" }, // ശ
    Sym { c: '\u{0d37}', kind: Kind::Consonant, iso: "ṣ", readable: "sh" }, // ഷ
    Sym { c: '\u{0d38}', kind: Kind::Consonant, iso: "s", readable: "s" }, // സ
    Sym { c: '\u{0d39}', kind: Kind::Consonant, iso: "h", readable: "h" }, // ഹ
    Sym { c: '\u{0d33}', kind: Kind::Consonant, iso: "ḷ", readable: "l'" }, // ള
    Sym { c: '\u{0d34}', kind: Kind::Consonant, iso: "ḻ", readable: "zh" }, // ഴ
    Sym { c: '\u{0d31}', kind: Kind::Consonant, iso: "ṟ", readable: "r'" }, // റ
    Sym { c: '\u{0d29}', kind: Kind::Consonant, iso: "ṉ", readable: "n" }, // ഩ
    Sym { c: '\u{0d01}', kind: Kind::Chandrabindu, iso: "m̐", readable: "M" }, // ഁ
    Sym { c: '\u{0d02}', kind: Kind::Anusvara, iso: "ṁ", readable: "M" }, // ം
    Sym { c: '\u{0d03}', kind: Kind::Visarga, iso: "ḥ", readable: "h'" }, // ഃ
    Sym { c: '\u{0964}', kind: Kind::Danda, iso: ".", readable: "." }, // ।
    Sym { c: '\u{0965}', kind: Kind::Danda, iso: "..", readable: ".." }, // ॥
    Sym { c: '\u{0d66}', kind: Kind::Digit, iso: "0", readable: "0" }, // ൦
    Sym { c: '\u{0d67}', kind: Kind::Digit, iso: "1", readable: "1" }, // ൧
    Sym { c: '\u{0d68}', kind: Kind::Digit, iso: "2", readable: "2" }, // ൨
    Sym { c: '\u{0d69}', kind: Kind::Digit, iso: "3", readable: "3" }, // ൩
    Sym { c: '\u{0d6a}', kind: Kind::Digit, iso: "4", readable: "4" }, // ൪
    Sym { c: '\u{0d6b}', kind: Kind::Digit, iso: "5", readable: "5" }, // ൫
    Sym { c: '\u{0d6c}', kind: Kind::Digit, iso: "6", readable: "6" }, // ൬
    Sym { c: '\u{0d6d}', kind: Kind::Digit, iso: "7", readable: "7" }, // ൭
    Sym { c: '\u{0d6e}', kind: Kind::Digit, iso: "8", readable: "8" }, // ൮
    Sym { c: '\u{0d6f}', kind: Kind::Digit, iso: "9", readable: "9" }, // ൯
];
pub(super) static MALAYALAM_NUKTA: &[(char, &str, &str)] = &[];
