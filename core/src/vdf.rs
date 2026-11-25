//! Minimal parser for Valve's text KeyValues (VDF/ACF) format.

#[derive(Debug, Clone, PartialEq)]
pub enum Vdf {
    Str(String),
    Obj(Vec<(String, Vdf)>),
}

impl Vdf {
    /// Case-insensitive child lookup (Valve files are inconsistent about casing).
    pub fn get(&self, key: &str) -> Option<&Vdf> {
        match self {
            Vdf::Obj(items) => items
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v),
            Vdf::Str(_) => None,
        }
    }

    pub fn str(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Vdf::Str(s) => Some(s),
            Vdf::Obj(_) => None,
        }
    }

    pub fn entries(&self) -> &[(String, Vdf)] {
        match self {
            Vdf::Obj(items) => items,
            Vdf::Str(_) => &[],
        }
    }
}

/// Parses a document; the result is an object holding the top-level keys.
pub fn parse(src: &str) -> Vdf {
    let mut chars = src.chars().peekable();
    Vdf::Obj(parse_obj(&mut chars))
}

type It<'a> = std::iter::Peekable<std::str::Chars<'a>>;

fn parse_obj(it: &mut It) -> Vec<(String, Vdf)> {
    let mut items = Vec::new();
    loop {
        let Some(key) = next_token(it) else { break };
        let key = match key {
            Tok::Close => break,
            Tok::Open => continue,
            Tok::Str(s) => s,
        };
        match next_token(it) {
            Some(Tok::Str(v)) => items.push((key, Vdf::Str(v))),
            Some(Tok::Open) => items.push((key, Vdf::Obj(parse_obj(it)))),
            _ => break,
        }
    }
    items
}

enum Tok {
    Str(String),
    Open,
    Close,
}

fn next_token(it: &mut It) -> Option<Tok> {
    loop {
        let c = *it.peek()?;
        if c.is_whitespace() {
            it.next();
        } else if c == '/' {
            // `// comment` to end of line
            while let Some(c) = it.next() {
                if c == '\n' {
                    break;
                }
            }
        } else if c == '[' {
            // Conditional like [$WIN32]; ignore
            while let Some(c) = it.next() {
                if c == ']' {
                    break;
                }
            }
        } else {
            break;
        }
    }
    match it.next()? {
        '{' => Some(Tok::Open),
        '}' => Some(Tok::Close),
        '"' => {
            let mut s = String::new();
            while let Some(c) = it.next() {
                match c {
                    '"' => break,
                    '\\' => match it.next() {
                        Some('n') => s.push('\n'),
                        Some('t') => s.push('\t'),
                        Some(o) => s.push(o),
                        None => break,
                    },
                    o => s.push(o),
                }
            }
            Some(Tok::Str(s))
        }
        first => {
            // Unquoted token
            let mut s = String::from(first);
            while let Some(&c) = it.peek() {
                if c.is_whitespace() || c == '{' || c == '}' || c == '"' {
                    break;
                }
                s.push(c);
                it.next();
            }
            Some(Tok::Str(s))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_libraryfolders() {
        let src = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"C:\\Program Files (x86)\\Steam"
		"apps"
		{
			"228980"		"505611636"
			"250820"		"5821820184"
		}
	}
	"1" { "path" "D:\\Games" }
}"#;
        let doc = parse(src);
        let lf = doc.get("LibraryFolders").unwrap();
        assert_eq!(lf.entries().len(), 2);
        assert_eq!(lf.get("0").unwrap().str("path"), Some(r"C:\Program Files (x86)\Steam"));
        assert_eq!(lf.get("0").unwrap().get("apps").unwrap().entries().len(), 2);
        assert_eq!(lf.get("1").unwrap().str("path"), Some(r"D:\Games"));
    }

    #[test]
    fn parses_acf_with_comments() {
        let src = "// hi\n\"AppState\"\n{\n\t\"appid\"\t\t\"546560\"\n\t\"name\"\t\t\"Half-Life: Alyx\"\n}";
        let doc = parse(src);
        let app = doc.get("appstate").unwrap();
        assert_eq!(app.str("appid"), Some("546560"));
        assert_eq!(app.str("name"), Some("Half-Life: Alyx"));
    }
}
