//! Local, deterministic redaction of the user's audience material before any of it reaches a model.
//!
//! [`Redactor`] replaces [`REDACTION_TEXT`] with placeholders such as `[name]`, and counts what it hid. It uses fixed
//! patterns and a fixed list of common first names only: the same text always comes out the same, and nothing is
//! sent anywhere to decide. It errs on the side of hiding too much, because a quote that lost a word is still useful
//! and a leaked name is not. It is not complete: a name it does not know, mid-sentence or in lower case, stays, which
//! is why every preview shows the text as sent and the user can add words to hide.

use std::collections::HashSet;
use std::sync::OnceLock;

use indexmap::IndexMap;
use regex::{Captures, Regex};
use tellfold_models::REDACTION_CATEGORIES;
pub use tellfold_models::REDACTION_TEXT;
use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

/// What [`Redactor::redact`] hid, by category, in [`REDACTION_CATEGORIES`] order.
pub const CATEGORIES: [&str; 6] = {
    let mut keys = [""; 6];
    let mut i = 0;
    while i < keys.len() {
        keys[i] = REDACTION_CATEGORIES[i].0;
        i += 1;
    }
    keys
};

pub const NAME: &str = "[name]";
pub const EMAIL: &str = "[email]";
pub const PHONE: &str = "[phone]";
pub const ADDRESS: &str = "[address]";
pub const NUMBER: &str = "[number]";
pub const LINK: &str = "[link]";

/// Counts of what was hidden, by category (every category present, in [`CATEGORIES`] order).
pub type Redactions = IndexMap<String, i64>;

pub fn empty_counts() -> Redactions {
    CATEGORIES.iter().map(|c| (c.to_string(), 0)).collect()
}

/// Adds `more` into `total`.
pub fn add_counts(total: &mut Redactions, more: &Redactions) {
    for (k, v) in more {
        *total.entry(k.clone()).or_insert(0) += v;
    }
}

/// Replaces personal details with placeholders, and counts what it replaced. Built once with the user's own words to
/// hide (the names of staff, pupils or customers the fixed rules would not know), each hidden as `[name]` wherever
/// it appears as a whole word, in any case; then used for every text.
#[derive(Debug, Clone, Default)]
pub struct Redactor {
    extra: Option<Regex>,
}

impl Redactor {
    /// A redactor that also hides `extra`. Err when those words cannot be made into one pattern (too many or too
    /// long), so they are never dropped without a word.
    pub fn new(extra: &[String]) -> Result<Self, String> {
        Ok(Redactor { extra: extra_words(extra)? })
    }

    /// The text with personal details replaced by placeholders, and what was replaced.
    pub fn redact(&self, text: &str) -> (String, Redactions) {
        let mut counts = empty_counts();
        let mut text = text.to_string();
        let mut hide =
            |text: &mut String, re: &Regex, category: &str, replace: &dyn Fn(&Captures) -> Option<String>| {
                let mut n = 0;
                let out = re
                    .replace_all(text, |c: &Captures| match replace(c) {
                        Some(r) => {
                            n += 1;
                            r
                        }
                        None => c[0].to_string(),
                    })
                    .into_owned();
                *text = out;
                *counts.get_mut(category).expect("every category") += n;
            };
        let whole = |token: &'static str| move |_: &Captures| Some(token.to_string());

        // Machine-shaped details first, so the name rules below never see an email's local part or a URL's path.
        hide(&mut text, re_email(), "emails", &whole(EMAIL));
        hide(&mut text, re_link(), "links", &whole(LINK));
        hide(&mut text, re_handle(), "names", &|c| Some(format!("{}{NAME}", &c[1])));
        hide(&mut text, re_card(), "numbers", &|c| {
            let digits = c[0].chars().filter(char::is_ascii_digit).count();
            (13..=19).contains(&digits).then(|| NUMBER.to_string())
        });
        hide(&mut text, re_labelled_number(), "numbers", &|c| {
            c[3].chars().any(|ch| ch.is_ascii_digit()).then(|| format!("{}{}{NUMBER}", &c[1], &c[2]))
        });
        hide(&mut text, re_phone(), "phones", &|c| {
            let digits = c[0].chars().filter(char::is_ascii_digit).count();
            (9..=15).contains(&digits).then(|| PHONE.to_string())
        });
        hide(&mut text, re_street(), "addresses", &whole(ADDRESS));
        hide(&mut text, re_po_box(), "addresses", &whole(ADDRESS));
        hide(&mut text, re_uk_postcode(), "addresses", &whole(ADDRESS));
        hide(&mut text, re_us_zip(), "addresses", &|c| Some(format!("{}{ADDRESS}", &c[1])));
        hide(&mut text, re_code(), "numbers", &whole(NUMBER));
        hide(&mut text, re_long_digits(), "numbers", &whole(NUMBER));

        // Names: titles, introductions, greetings and sign-offs, signature lines, then known given names.
        hide(&mut text, re_titled(), "names", &whole(NAME));
        hide(&mut text, re_intro(), "names", &|c| Some(format!("{}{NAME}", &c[1])));
        hide(&mut text, re_this_is(), "names", &|c| Some(format!("{}{NAME}{}", &c[1], &c[2])));
        hide(&mut text, re_greeting(), "names", &|c| Some(format!("{}{NAME}{}", &c[1], &c[2])));
        hide(&mut text, re_dash_signature(), "names", &|c| Some(format!("{}{}{NAME}{}", &c[1], &c[2], &c[3])));
        hide(&mut text, re_initial_signature(), "names", &|c| Some(format!("{}{}{NAME}{}", &c[1], &c[2], &c[3])));
        hide(&mut text, re_inline_signature(), "names", &|c| Some(format!("{}{NAME}", &c[1])));
        let mut names = sign_off_names(&mut text);
        names += given_names_in(&mut text);
        if let Some(words) = &self.extra {
            hide(&mut text, words, "names", &whole(NAME));
        }
        *counts.get_mut("names").expect("names") += names;
        // a surname left after a hidden first name ("[name] Hollis") goes with it
        text = re_name_then_surname().replace_all(&text, NAME).into_owned();
        (text, counts)
    }
}

/// One pattern for the user's own words to hide: whole words, any case, longest first. A word that starts or ends
/// with punctuation ("J.R.", "Smith & Co.") is bounded only on its word-character side.
fn extra_words(extra: &[String]) -> Result<Option<Regex>, String> {
    let mut words: Vec<String> = extra
        .iter()
        .map(|w| w.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|w| w.chars().count() >= 2)
        .collect();
    words.sort_by_key(|w| std::cmp::Reverse(w.chars().count()));
    words.dedup();
    if words.is_empty() {
        return Ok(None);
    }
    let is_word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    let alternatives: Vec<String> = words
        .iter()
        .map(|w| {
            let start = if is_word(w.chars().next()) { r"\b" } else { "" };
            let end = if is_word(w.chars().last()) { r"\b" } else { "" };
            format!("{start}{}{end}", regex::escape(w).replace(' ', r"\s+"))
        })
        .collect();
    let pattern = format!(r"(?i)(?:{})", alternatives.join("|"));
    Regex::new(&pattern).map(Some).map_err(|e| match e {
        regex::Error::CompiledTooBig(_) => "there are too many of them, or they are too long".to_string(),
        other => other.to_string(),
    })
}

pattern!(re_email, r"(?i)\b[\p{L}\p{N}][\p{L}\p{N}._%+-]*@[\p{L}\p{N}-]+(?:\.[\p{L}\p{N}-]+)*\.\p{L}{2,}\b");
pattern!(re_link, r#"(?i)\b(?:https?://|www\.)[^\s<>"'\])]+"#);
pattern!(re_handle, r"(^|[^\w@\[])@[A-Za-z0-9_](?:[A-Za-z0-9_.]{0,28}[A-Za-z0-9_])\b");
pattern!(
    re_labelled_number,
    r"(?i)(\b(?:order|account|acct|a/c|invoice|ticket|case|ref|reference|customer|client|member|membership|booking|reservation|policy|tracking|transaction|confirmation|receipt|card|iban|sort code|routing|subscription|licen[cs]e|serial|patient|student|employee|staff)\b)(\s*(?:(?:no\.?|number|num|nr|id|#|code|ref)\s*(?:[:#.]|is|was)?|[:#.])?\s*)([A-Z0-9][A-Z0-9/-]{2,})\b"
);
pattern!(re_card, r"\b\d(?:[ -]?\d){12,18}\b");
pattern!(re_phone, r"(?:\+\(?|\(|\b)\d[\d ().-]{7,}\d\b");
pattern!(
    re_street,
    r"\b(?:(?i:flat|apt\.?|apartment|unit|suite)\s*\d+[A-Za-z]?,?\s+)?\d{1,5}[A-Za-z]?,?\s+(?:[\p{Lu}][\p{L}'’.-]*\s+){1,4}(?i:street|st|road|rd|avenue|ave|lane|ln|drive|dr|boulevard|blvd|way|court|ct|place|pl|close|crescent|cres|terrace|square|sq|highway|hwy|parkway|pkwy|gardens|grove|row|mews|circle|cir|trail|walk)\b\.?"
);
pattern!(re_po_box, r"(?i)\bp\.?\s?o\.?\s+box\s+\d+\b");
pattern!(re_uk_postcode, r"\b[A-Z]{1,2}\d[A-Z\d]?\s*\d[A-Z]{2}\b");
pattern!(re_us_zip, r"(\b[A-Z]{2}\s+)\d{5}(?:-\d{4})?\b");
pattern!(re_code, r"\b[A-Z]{1,5}-?\d{4,}[A-Z0-9]*\b");
pattern!(re_long_digits, r"\b\d{6,}\b");
pattern!(
    re_titled,
    r"\b(?:Mr|Mrs|Ms|Miss|Mx|Dr|Prof|Professor|Sir|Dame|Lady|Lord|Rev|Fr|Sr|Sra|Herr|Frau|Mme|Mlle|Mons)\.?\s+(?:[\p{Lu}]\.\s*)?[\p{Lu}][\p{L}'’-]+(?:\s+[\p{Lu}][\p{L}'’-]+)?"
);
pattern!(
    re_intro,
    r"(?i)(\b(?:my name is|my name's|i am called|i'm called|name:|signed:?|from:|to:|cc:|regards from)\s+)(?-i:[\p{Lu}][\p{L}'’-]+(?:\s+(?:[\p{Lu}]\.|[\p{Lu}][\p{L}'’-]+)){0,2})"
);
pattern!(
    re_this_is,
    r"(?i)(\bthis is\s+)(?-i:[\p{Lu}][\p{L}'’-]+(?:\s+[\p{Lu}][\p{L}'’-]+)?)(\s+(?:from|here|at|with)\b)"
);
pattern!(
    re_greeting,
    r"(?im)(^[ \t]*(?:hi|hello|hey|dear|morning|good morning|good afternoon|good evening|thanks|thank you|many thanks|cheers|regards|best regards|kind regards|warm regards|best|sincerely|yours sincerely|yours truly)[ \t,]+)(?-i:[\p{Lu}][\p{L}'’-]+(?:[ \t]+(?:[\p{Lu}]\.|[\p{Lu}][\p{L}'’-]+)){0,2})([ \t]*(?:[,!.:;]|$))"
);
// signatures are the last line of an item, so a bullet list of capitalised topics is left alone
pattern!(
    re_dash_signature,
    r"(\A|\n)([ \t]*[-–—~][ \t]*)[\p{Lu}][\p{L}'’-]+(?:[ \t]+(?:[\p{Lu}]\.?|[\p{Lu}][\p{L}'’.-]+)){0,2}([ \t]*\s*)\z"
);
pattern!(re_initial_signature, r"(\A|\n)([ \t]*[-–—~]?[ \t]*)[\p{Lu}]\.[ \t]?[\p{Lu}][\p{L}'’-]+\.?([ \t]*\s*)\z");
// "... on time. — Tamsin G." at the very end of an item
pattern!(
    re_inline_signature,
    r"(\s[-–—~]{1,2}[ \t]*)[\p{Lu}][\p{L}'’-]+(?:[ \t]+(?:[\p{Lu}]\.?|[\p{Lu}][\p{L}'’.-]+)){0,2}\.?[ \t]*\s*\z"
);
pattern!(re_name_then_surname, r"\[name\](?:[ \t]+[\p{Lu}][\p{L}'’-]+){1,2}\b");
pattern!(re_capitalised, r"\b[\p{Lu}][\p{L}'’-]+");
pattern!(re_surname_after, r"^[ \t]+(?:[\p{Lu}]\.|[\p{Lu}][\p{L}'’-]+)");
pattern!(
    re_sign_off_line,
    r"(?i)^\s*(?:thanks|thank you|many thanks|thanks again|cheers|regards|best regards|kind regards|warm regards|best wishes|best|all the best|sincerely|yours sincerely|yours faithfully|yours truly|love|take care|talk soon)\s*[,!.]*\s*$"
);
pattern!(re_name_line, r"^\s*[\p{Lu}][\p{L}'’.-]*(?:\s+[\p{Lu}][\p{L}'’.-]*){0,3}\s*$");

/// The one or two short lines under a sign-off ("Thanks,\nSarah Jones") are a name.
fn sign_off_names(text: &mut String) -> i64 {
    let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    let mut n = 0;
    let mut i = 0;
    while i < lines.len() {
        if re_sign_off_line().is_match(&lines[i]) {
            let mut j = i + 1;
            while j < lines.len() && lines[j].trim().is_empty() && j <= i + 1 {
                j += 1;
            }
            // the name, and a second line under it (a surname or a job title next to it is left alone)
            if j < lines.len() && re_name_line().is_match(&lines[j]) && lines[j].trim() != NAME {
                let indent: String = lines[j].chars().take_while(|c| c.is_whitespace()).collect();
                lines[j] = format!("{indent}{NAME}");
                n += 1;
            }
            i = j;
        }
        i += 1;
    }
    if n > 0 {
        *text = lines.join("\n");
    }
    n
}

/// Known given names, with the capitalised surname or initial after one: "[name]" each, a possessive kept. A given
/// name that is also an everyday word ("Will", "May", "Grace") only counts with a surname after it.
fn given_names_in(text: &mut String) -> i64 {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut n = 0;
    let mut skip_to = 0;
    for m in re_capitalised().find_iter(text) {
        if m.start() < skip_to {
            continue;
        }
        let word = m.as_str();
        let (stem, possessive) = match word.strip_suffix("'s").or_else(|| word.strip_suffix("’s")) {
            Some(stem) => (stem, &word[stem.len()..]),
            None => (word, ""),
        };
        let lower = fold(stem);
        if !given_names().contains(lower.as_str()) {
            continue;
        }
        let surname = if possessive.is_empty() { re_surname_after().find(&text[m.end()..]) } else { None };
        if surname.is_none() && AMBIGUOUS_NAMES.contains(&lower.as_str()) {
            continue;
        }
        let end = m.end() + surname.map_or(0, |s| s.end());
        // a surname's own possessive stays too
        let tail = &text[m.start()..end];
        let kept = if !possessive.is_empty() {
            possessive
        } else if tail.ends_with("'s") {
            "'s"
        } else if tail.ends_with("’s") {
            "’s"
        } else {
            ""
        };
        out.push_str(&text[last..m.start()]);
        out.push_str(NAME);
        out.push_str(kept);
        last = end;
        skip_to = end;
        n += 1;
    }
    if n > 0 {
        out.push_str(&text[last..]);
        *text = out;
    }
    n
}

/// Lower case without accents, so "José" and "Chloé" find "jose" and "chloe".
fn fold(word: &str) -> String {
    word.to_lowercase().nfkd().filter(|c| !is_combining_mark(*c)).collect()
}

fn given_names() -> &'static HashSet<&'static str> {
    static S: OnceLock<HashSet<&'static str>> = OnceLock::new();
    S.get_or_init(|| GIVEN_NAMES.split_whitespace().chain(AMBIGUOUS_NAMES.iter().copied()).collect())
}

/// Given names that are also ordinary words, months or places: hidden only when a surname follows.
const AMBIGUOUS_NAMES: &[&str] = &[
    "will",
    "may",
    "june",
    "april",
    "august",
    "mark",
    "rose",
    "grace",
    "hope",
    "faith",
    "joy",
    "bill",
    "pat",
    "art",
    "sue",
    "rob",
    "jack",
    "frank",
    "chase",
    "max",
    "summer",
    "dawn",
    "penny",
    "holly",
    "ivy",
    "lily",
    "jade",
    "ruby",
    "crystal",
    "amber",
    "sky",
    "river",
    "brook",
    "hunter",
    "mason",
    "carter",
    "cooper",
    "taylor",
    "parker",
    "jordan",
    "victoria",
    "georgia",
    "sydney",
    "paris",
    "florence",
    "virginia",
    "carolina",
    "india",
    "china",
    "dakota",
    "austin",
    "dallas",
    "houston",
    "phoenix",
    "eve",
    "gene",
    "guy",
    "ken",
    "lane",
    "miles",
    "ray",
    "rich",
    "sterling",
    "wade",
    "angel",
    "harmony",
    "heaven",
    "honey",
    "patience",
    "prudence",
    "royal",
    "jan",
    "milan",
    "christian",
    "morgan",
    "dean",
    "don",
    "iris",
    "carol",
    "mario",
    "rita",
    "theo",
    "leo",
    "jean",
    "kim",
    "lee",
];

/// Common given names across English, European, South Asian, Middle Eastern, East Asian and Armenian usage, lower
/// case without accents (see [`fold`]), none of them also in [`AMBIGUOUS_NAMES`]. Not exhaustive: the other rules
/// catch names in greetings, sign-offs and introductions, and a name in neither stays.
const GIVEN_NAMES: &str = "
aaron abby abdul abdullah abigail adam adele adrian ahmad ahmed aidan aisha alan alana albert alberto alejandro
alex alexa alexander alexandra alexis alfie alfred ali alice alicia alison amanda amelia amir amit amy ana anahit
anastasia andre andrea andreas andrei andrew andy angela angelica angus anil anita ann anna annabel anne annie
anthony antonio anya aram archie arjun armen arthur arturo arun ashley ashot astrid audrey ava avery barbara barry
beatrice becky ben benjamin bernard beth bethany betty beverly bianca bob bobby bonnie brad bradley brandon brenda
brendan brian bridget britney bruce bruno bryan caitlin caleb callum calvin cameron camila carl carla carlos carmen
caroline carrie casey catherine cathy cecilia chad charles charlie charlotte chen cheryl chloe chris
christina christine christopher cindy claire clara claudia colin connor conor craig cristina curtis cynthia daisy
dale damian dan dana daniel daniela danielle danny darius darren dave david davit debbie deborah declan deepak
denise dennis derek diana diane diego dimitri dominic donald donna doris dorothy douglas duncan dylan eddie
edgar edith edward eileen elaine eleanor elena eli elijah elizabeth ella ellen ellie elliot emil emily emma enrique
eric erica erik erin ernest esther ethan eugene eva evan evelyn fahad farah fatima felipe felix fernando fiona
francesca francesco francis francisco freddie frederick freya gabriel gabriela gabrielle gareth gary gavin gemma
geoffrey george gerald gerard gina giovanni giulia gloria gordon graham greg gregory gustavo hailey hamid hana
hannah hans harold harriet harry harvey hassan hayk heather hector heidi helen helena henry hilary howard hugh hugo
hussein ian ibrahim igor imran ines ingrid irene isaac isabel isabella isabelle ivan jacob jacqueline jaime
jake james jamie jane janet janice jared jasmine jason javier jay jeff jeffrey jen jenna jennifer jenny
jeremy jerry jess jessica jesus jill jim jimmy joan joanna joanne joe joel johann johanna john johnny jon jonathan
jorge jose josef joseph josephine josh joshua juan judith judy julia julian julie juliet justin kai kamal karen
karim karina karl kate katherine kathleen kathryn kathy katie kayla keith kelly kenneth kevin khalid kimberly
kirsty kristen kumar kyle lara laura lauren leah leila lena leon leonardo lewis liam lilit linda lindsay
lisa liz lorenzo louis louise lucas lucia lucy luis luke lydia lynn maddie madison maggie mahmoud malcolm manuel
marc marcel marco marcus margaret maria mariam marie marina marisa marta martha martin marty mary maryam
matilda matt matteo matthew maureen maurice maya megan mehmet melanie melissa mia michael michelle miguel mike
mila mohamed mohammad mohammed molly monica muhammad nadia nancy naomi narek natalia natalie natasha
nathan neil nicholas nick nicola nicole nikhil nina noah noel nora norman oliver olivia omar oscar owen pablo
pamela paolo patricia patrick paul paula pauline pedro peter philip phillip phoebe pierre pooja priya rachel rafael
rahul raj rajesh ralph ramon randy rashid raymond rebecca reza ricardo richard rick ricky robert roberta
roberto robin rodrigo roger roland ronald rosa rosie ross roy russell ruth ryan sabrina sally sam samantha samir
samuel sandra sanjay sara sarah sasha scott sean sebastian sergei sergio seth shannon sharon shaun shawn sheila
shirley simon sofia sonia sophia sophie stacy stella stephanie stephen steve steven stuart susan suzanne sven tamar
tanya tara ted teresa terry theodore theresa thomas tiffany tim timothy tina toby todd tom tommy tony tracy
travis trevor tyler valentina valeria vanessa vera veronica victor vincent vivian walter wang wayne wei wendy
william yasmin yusuf yvonne zach zachary zara zoe
";

#[cfg(test)]
mod tests {
    use super::*;

    fn redact(text: &str) -> (String, Redactions) {
        Redactor::default().redact(text)
    }

    fn redact_with(text: &str, extra: &[String]) -> (String, Redactions) {
        Redactor::new(extra).unwrap().redact(text)
    }

    fn hidden(text: &str) -> String {
        redact(text).0
    }

    fn count(text: &str, category: &str) -> i64 {
        redact(text).1[category]
    }

    #[test]
    fn emails_and_links() {
        assert_eq!(hidden("Write to jane.doe+news@example.co.uk today"), "Write to [email] today");
        assert_eq!(hidden("see https://shop.example.com/u/829?id=4 or www.example.org/me"), "see [link] or [link]");
        assert_eq!(count("a@b.io and c.d@e.fr", "emails"), 2);
    }

    #[test]
    fn handles_are_names() {
        assert_eq!(hidden("thanks @maria_dev!"), "thanks [name]!");
        assert_eq!(hidden("@bob_k said so"), "[name] said so");
        assert_eq!(count("@bob_k said so", "names"), 1);
    }

    #[test]
    fn phone_numbers() {
        for phone in ["+44 7700 900123", "(555) 123-4567", "555.123.4567", "07700 900 123", "+1-202-555-0175"] {
            assert_eq!(hidden(&format!("call {phone} now")), "call [phone] now", "{phone}");
        }
    }

    #[test]
    fn dates_prices_and_years_stay() {
        for text in [
            "Between 2023-2024 the price went from 4.99 to 7.99.",
            "It costs $120 a year, up 25%.",
            "On 12.03.2024 at 10:30 it crashed 3 times.",
            "Version 2.10.1 fixed it.",
            "the subscription was 120 a year",
            "my membership is 2025 pricing",
        ] {
            assert_eq!(hidden(text), text);
        }
    }

    #[test]
    fn order_and_account_numbers() {
        assert_eq!(hidden("My order #A12345 never came"), "My order #[number] never came");
        assert_eq!(hidden("Order number: 88213 was late"), "Order number: [number] was late");
        assert_eq!(hidden("account no. 4433-221"), "account no. [number]");
        assert_eq!(hidden("ticket INC-0042981 is open"), "ticket [number] is open");
        assert_eq!(hidden("ref ORD-2024-11873"), "ref [number]");
        assert_eq!(hidden("charged twice, see 99812345"), "charged twice, see [number]");
        assert_eq!(hidden("card 4111 1111 1111 1111 declined"), "card [number] declined");
        assert_eq!(count("order #A12345 and account 99881", "numbers"), 2);
        // the word alone, or a number without digits, is not an id
        assert_eq!(hidden("I placed an order last week"), "I placed an order last week");
        assert_eq!(hidden("my account settings"), "my account settings");
    }

    #[test]
    fn street_addresses_and_postcodes() {
        assert_eq!(hidden("I live at 221B Baker Street, London"), "I live at [address], London");
        assert_eq!(hidden("send it to 1600 Pennsylvania Avenue NW"), "send it to [address] NW");
        assert_eq!(hidden("Flat 3, 12 Mill Lane flooded"), "[address] flooded");
        assert_eq!(hidden("postcode SW1A 1AA"), "postcode [address]");
        assert_eq!(hidden("Springfield, IL 62704"), "Springfield, IL [address]");
        assert_eq!(hidden("PO Box 4421"), "[address]");
        assert_eq!(count("12 Mill Lane and 4 Oak Road", "addresses"), 2);
    }

    #[test]
    fn titled_and_introduced_names() {
        assert_eq!(hidden("Mrs Patel was lovely"), "[name] was lovely");
        assert_eq!(hidden("ask Dr. J. Okafor"), "ask [name]");
        assert_eq!(hidden("Hi, my name is Tamsin Greaves and"), "Hi, my name is [name] and");
        assert_eq!(hidden("this is Kofi from the PTA"), "this is [name] from the PTA");
    }

    #[test]
    fn greetings_and_sign_offs() {
        assert_eq!(hidden("Hi Sarah,\nThe app is slow."), "Hi [name],\nThe app is slow.");
        assert_eq!(hidden("Dear Ms Novak,"), "Dear [name],");
        assert_eq!(hidden("Thanks, Priyanka"), "Thanks, [name]");
        assert_eq!(
            hidden("It broke again.\n\nKind regards,\nTamsin Greaves\n"),
            "It broke again.\n\nKind regards,\n[name]\n"
        );
        assert_eq!(hidden("Cheers\nOlu"), "Cheers\n[name]");
    }

    #[test]
    fn review_signatures() {
        assert_eq!(hidden("Great value.\n— Siobhan K."), "Great value.\n— [name]");
        assert_eq!(hidden("Too pricey\n- J. Smith"), "Too pricey\n- [name]");
    }

    #[test]
    fn signatures_at_the_end_of_a_line_of_text() {
        assert_eq!(hidden("The fare went up twice. — Tamsin G."), "The fare went up twice. — [name]");
        assert_eq!(hidden("Lovely boats - Orla"), "Lovely boats - [name]");
        // a dash inside the text is left alone
        assert_eq!(hidden("Two boats - Blue and Red - run late"), "Two boats - Blue and Red - run late");
    }

    #[test]
    fn a_surname_after_a_hidden_name_goes_too() {
        let extra = vec!["brennan".to_string()];
        assert_eq!(redact_with("my name is Brennan Hollis and", &extra).0, "my name is [name] and");
        assert_eq!(redact_with("Brennan Hollis rang", &extra).0, "[name] rang");
    }

    #[test]
    fn known_given_names() {
        assert_eq!(
            hidden("I asked Michael about it and Laura Chen agreed"),
            "I asked [name] about it and [name] agreed"
        );
        assert_eq!(hidden("Zoë said no"), "[name] said no");
        assert_eq!(hidden("Narek's kids love it"), "[name]'s kids love it");
        assert_eq!(hidden("Yesterday Michael said no"), "Yesterday [name] said no");
        assert_eq!(hidden("Ask Laura Chen's team"), "Ask [name]'s team");
    }

    #[test]
    fn everyday_words_that_are_names() {
        for text in ["Will this work in May?", "Grace periods are too short.", "I hope it lasts.", "Mark my words."] {
            assert_eq!(hidden(text), text);
        }
        assert_eq!(hidden("We spoke to Will Harper"), "We spoke to [name]");
    }

    #[test]
    fn deterministic_and_idempotent() {
        let text = "Hi Sarah, order #A12345 for 12 Mill Lane, call +44 7700 900123 or sarah@x.com. — Sarah K.";
        let (once, counts) = redact(text);
        assert_eq!(redact(text), (once.clone(), counts.clone()));
        assert_eq!(redact(&once).0, once, "placeholders survive a second pass");
        assert!(redact(&once).1.values().all(|n| *n == 0), "nothing left to hide: {:?}", redact(&once).1);
        for leak in ["Sarah", "A12345", "Mill Lane", "7700", "sarah@"] {
            assert!(!once.contains(leak), "{leak} leaked: {once}");
        }
    }

    #[test]
    fn the_users_own_words() {
        let extra = vec!["Tamsin".to_string(), "Oakfield  Primary".to_string(), "x".to_string()];
        let (text, counts) = redact_with("tamsin said Oakfield Primary is late; Tamsinx no", &extra);
        assert_eq!(text, "[name] said [name] is late; Tamsinx no");
        assert_eq!(counts["names"], 2);
    }

    #[test]
    fn topic_bullets_are_not_signatures() {
        let notes = "- Budget Review\n- Bus Times\nWe agreed to wait.";
        assert_eq!(hidden(notes), notes);
    }

    #[test]
    fn everyday_distances_are_not_addresses() {
        for text in ["It is 5 minutes down the road.", "After 2 weeks the park closed.", "I walked 3 miles home."] {
            assert_eq!(hidden(text), text);
        }
    }

    #[test]
    fn unlisted_names_mid_sentence_stay() {
        // the documented limit: a name the rules do not know, outside a greeting, sign-off, title or introduction,
        // is sent as written, so the preview and the words to hide matter
        for text in ["Orla at the ticket desk was rude", "sarah said it was fine"] {
            assert_eq!(hidden(text), text);
        }
    }

    #[test]
    fn accented_names() {
        assert_eq!(hidden("José and Chloé came back"), "[name] and [name] came back");
        assert_eq!(hidden("Zoe said no"), "[name] said no");
    }

    #[test]
    fn emails_beyond_ascii() {
        assert_eq!(hidden("write zoë@example.com or info@müller-bücher.de"), "write [email] or [email]");
    }

    #[test]
    fn numbers_after_is() {
        assert_eq!(hidden("my order number is 88213, sorry"), "my order number is [number], sorry");
        assert_eq!(hidden("the account number is 99881"), "the account number is [number]");
    }

    #[test]
    fn words_to_hide_with_punctuation() {
        let extra = vec!["J.R.".to_string(), "Smith & Co.".to_string()];
        assert_eq!(redact_with("J.R. said Smith & Co. were late", &extra).0, "[name] said [name] were late");
    }

    #[test]
    fn words_to_hide_that_cannot_be_used_are_an_error() {
        let words: Vec<String> = (0..2000).map(|i| format!("{i}{}", "verylongname".repeat(40))).collect();
        let err = Redactor::new(&words).unwrap_err();
        assert!(err.contains("too many") || err.contains("too long"), "{err}");
    }

    #[test]
    fn dotted_handles() {
        assert_eq!(hidden("thanks @tamsin.greaves."), "thanks [name].");
    }

    #[test]
    fn one_redactor_for_many_texts() {
        let redactor = Redactor::new(&["Mossbook".to_string()]).unwrap();
        assert_eq!(redactor.redact("Mossbook is slow").0, "[name] is slow");
        assert_eq!(redactor.redact("mossbook again").0, "[name] again");
    }

    #[test]
    fn counts_have_every_category_in_order() {
        let (_, counts) = redact("nothing personal here");
        assert_eq!(counts.keys().map(String::as_str).collect::<Vec<_>>(), CATEGORIES);
    }
}
