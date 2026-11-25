//! Deterministic `basic` cleaning of dictated text. Only fillers, immediate word repeats, spacing,
//! the first capital letter and the final period change; every other word is left as spoken.

/// One whitespace-delimited word split into leading punctuation, the alphanumeric core and
/// trailing punctuation. Tokens with no alphanumeric character have an empty core.
struct Token {
    lead: String,
    core: String,
    trail: String,
}

impl Token {
    fn new(word: &str) -> Self {
        let Some(start) = word.find(char::is_alphanumeric) else {
            return Self {
                lead: word.to_owned(),
                core: String::new(),
                trail: String::new(),
            };
        };
        let end = word
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_alphanumeric())
            .map_or(word.len(), |(index, c)| index + c.len_utf8());
        Self {
            lead: word[..start].to_owned(),
            core: word[start..end].to_owned(),
            trail: word[end..].to_owned(),
        }
    }

    fn ends_sentence(&self) -> bool {
        self.trail.ends_with(['.', '!', '?'])
    }
}

/// Collapses runs of one letter ("hmmm" -> "hm") so spelling variants share a form.
fn collapse_runs(word: &str) -> String {
    let mut out = String::new();
    for c in word.chars() {
        if !out.ends_with(c) {
            out.push(c);
        }
    }
    out
}

fn is_filler(core: &str) -> bool {
    let lower = core.to_lowercase();
    match collapse_runs(&lower).as_str() {
        "um" | "uh" | "uhm" | "erm" | "ah" | "hm" => true,
        "er" => lower == "er",
        "m" => lower.len() >= 2,
        _ => false,
    }
}

fn is_plain_word(core: &str) -> bool {
    core.chars()
        .all(|c| c.is_alphabetic() || c == '\'' || c == '\u{2019}')
}

fn uppercase_first_letter(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut done = false;
    for c in text.chars() {
        if !done && c.is_alphabetic() {
            out.extend(c.to_uppercase());
            done = true;
        } else {
            out.push(c);
            done = done || c.is_alphanumeric();
        }
    }
    out
}

pub fn basic(text: &str) -> String {
    let mut tokens: Vec<Token> = Vec::new();
    for word in text.split_whitespace() {
        let token = Token::new(word);
        let punctuation_only = token.core.is_empty()
            && word
                .chars()
                .all(|c| matches!(c, ',' | '.' | '!' | '?' | ';' | ':' | '\u{2026}'));
        match tokens.last_mut() {
            Some(previous) if punctuation_only => previous.trail.push_str(word),
            _ => tokens.push(token),
        }
    }

    let mut kept: Vec<Token> = Vec::with_capacity(tokens.len());
    let mut capitalise_next = false;
    for token in tokens {
        let sentence_start = kept.last().is_none_or(Token::ends_sentence);
        if token.lead.is_empty() && !token.core.is_empty() && is_filler(&token.core) {
            // Drop the filler and its trailing commas; a lone ".", "!" or "?" ends the sentence
            // that came before it ("so, um." -> "so."), an ellipsis is just a pause and is dropped.
            let terminal = token
                .trail
                .trim_end_matches(['"', '\'', ')'])
                .chars()
                .last();
            let pause = token.trail.contains("..") || token.trail.contains('\u{2026}');
            if let (Some(mark @ ('.' | '!' | '?')), Some(previous), false) =
                (terminal, kept.last_mut(), pause)
                && !previous.ends_sentence()
            {
                previous.trail = previous.trail.trim_end_matches([',', ';', ':']).to_owned();
                previous.trail.push(mark);
            }
            capitalise_next |= sentence_start;
            continue;
        }

        let mut token = token;
        if capitalise_next {
            capitalise_next = false;
            token.core = uppercase_first_letter(&token.core);
        }
        if let Some(previous) = kept.last_mut() {
            let repeats = previous.trail.is_empty()
                && token.lead.is_empty()
                && is_plain_word(&token.core)
                && !token.core.is_empty()
                && previous.core.to_lowercase() == token.core.to_lowercase();
            if repeats {
                previous.trail = token.trail;
                continue;
            }
        }
        kept.push(token);
    }

    let mut out = kept
        .iter()
        .map(|t| format!("{}{}{}", t.lead, t.core, t.trail))
        .collect::<Vec<_>>()
        .join(" ");
    if out.is_empty() {
        return out;
    }
    out = uppercase_first_letter(&out);
    let closing: &[char] = &['"', '\'', ')', ']', '\u{201d}', '\u{2019}'];
    let ends_with_terminal = out
        .trim_end_matches(closing)
        .ends_with(['.', '!', '?', '\u{2026}']);
    if !ends_with_terminal {
        let trimmed = out.trim_end_matches([',', ';', ':']).len();
        out.truncate(trimmed);
        out.push('.');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::basic;

    #[test]
    fn basic_cleaning_table() {
        let cases = [
            // fillers and their trailing commas
            ("um, hello there", "Hello there."),
            ("Um hello", "Hello."),
            ("so uh I think", "So I think."),
            ("I think, um, we should go", "I think, we should go."),
            ("erm let me see", "Let me see."),
            ("ah, right", "Right."),
            ("hmm that is odd", "That is odd."),
            ("mm yes", "Yes."),
            ("UMMM okay", "Okay."),
            ("it is, uh.", "It is."),
            ("well, um... I guess", "Well, I guess."),
            ("Hello. Um, how are you", "Hello. How are you."),
            ("uh", ""),
            ("um, uh, erm", ""),
            // words that merely look like fillers stay
            ("the hum of the err humming", "The hum of the err humming."),
            (
                "umbrella and ahead of ermine",
                "Umbrella and ahead of ermine.",
            ),
            ("M and m are letters", "M and m are letters."),
            // immediate repeats, case-insensitive
            ("I I think the the plan works", "I think the plan works."),
            ("The the cat", "The cat."),
            ("no no no", "No."),
            ("I, um, I mean it", "I, I mean it."),
            ("um um hello", "Hello."),
            ("so so, good", "So, good."),
            ("1 1 is not collapsed", "1 1 is not collapsed."),
            // spacing and punctuation
            ("hello   world", "Hello world."),
            ("hello , world !", "Hello, world!"),
            ("wait\n\nwhat", "Wait what."),
            ("is it done ?", "Is it done?"),
            // capitalisation and final punctuation
            ("this is fine", "This is fine."),
            ("already done.", "Already done."),
            ("really?", "Really?"),
            ("stop!", "Stop!"),
            ("he said \"go.\"", "He said \"go.\""),
            ("and then,", "And then."),
            ("wait...", "Wait..."),
            ("\u{201c}quoted start", "\u{201c}Quoted start."),
            ("i like the iPhone", "I like the iPhone."),
            // other words are never changed
            ("so i i think", "So i think."),
            ("send 3.5 apples to NASA", "Send 3.5 apples to NASA."),
            ("", ""),
            ("   ", ""),
        ];
        for (input, expected) in cases {
            assert_eq!(basic(input), expected, "input: {input:?}");
        }
    }

    #[test]
    fn cleaning_is_idempotent() {
        for input in [
            "um, so uh the the plan, erm, works",
            "hello , world !",
            "I I I mean it",
        ] {
            let once = basic(input);
            assert_eq!(basic(&once), once, "{input:?}");
        }
    }
}
