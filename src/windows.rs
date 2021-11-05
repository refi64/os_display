use core::fmt::{self, Formatter, Write};

use unicode_width::UnicodeWidthChar;

// Much of this code is similar to the Unix version.
// Not all comments are repeated, so read that first.

/// I'm not too familiar with PowerShell, much of this is based on
/// experimentation rather than documentation or deep understanding.
/// I have noticed that ~?*[] only get expanded in some contexts, so watch
/// out for that if doing your own tests.
/// Get-ChildItem seems unwilling to quote anything so it doesn't help.
/// The omission of \ is important because it's used in file paths.
const SPECIAL_SHELL_CHARS: &[u8] = b"|&;<>()$`\"'*?[]=,{} ";

/// A single stand-alone exclamation mark seems to have some special meaning.
/// Tildes are unclear: In Powershell on Linux, quoting a tilde keeps it from
/// expanding if passed to an external program, but not if passed to Get-ChildItem.
const SPECIAL_SHELL_CHARS_START: &[char] = &['~', '#', '@', '!'];

const DOUBLE_UNSAFE: &[u8] = &[b'"', b'`', b'$'];

pub(crate) fn write(f: &mut Formatter<'_>, text: &str, force_quote: bool) -> fmt::Result {
    let mut is_single_safe = true;
    let mut is_double_safe = true;
    let mut requires_quote = force_quote;
    let mut is_bidi = false;

    if !requires_quote {
        if let Some(first) = text.chars().next() {
            if SPECIAL_SHELL_CHARS_START.contains(&first) {
                requires_quote = true;
            }

            // PowerShell may parse bare strings as numbers in some contexts.
            // `echo 1d` just outputs "1d", but `Set-Variable s 1d` assigns
            // the number 1 to s.
            if !requires_quote && first.is_ascii_digit() {
                requires_quote = true;
            }

            // Annoyingly, .0d is another example.
            // And filenames start with . commonly enough that we shouldn't quote
            // too eagerly.
            if !requires_quote && first == '.' {
                if let Some(second) = text.chars().nth(1) {
                    if second.is_ascii_digit() {
                        requires_quote = true;
                    }
                }
            }

            // (Negative numbers are covered below and seem to be treated differently anyway.)

            // Unlike in Unix, quoting an argument may stop it
            // from being recognized as an option. I like that very much.
            // But we don't want to quote "-" because that's a common
            // special argument and PowerShell doesn't mind it.
            if !requires_quote && unicode::is_dash(first) && text.len() > 1 {
                requires_quote = true;
            }

            if !requires_quote && first.width().unwrap_or(0) == 0 {
                requires_quote = true;
            }
        } else {
            requires_quote = true;
        }
    }

    for ch in text.chars() {
        if ch.is_ascii() {
            let ch = ch as u8;
            if ch == b'\'' {
                is_single_safe = false;
            }
            if is_double_safe && DOUBLE_UNSAFE.contains(&ch) {
                is_double_safe = false;
            }
            if !requires_quote && SPECIAL_SHELL_CHARS.contains(&ch) {
                requires_quote = true;
            }
            if ch.is_ascii_control() {
                return write_escaped(f, text.chars().map(Ok));
            }
        } else {
            if !requires_quote && unicode::is_whitespace(ch) {
                requires_quote = true;
            }
            if (!requires_quote || is_double_safe) && unicode::is_double_quote(ch) {
                is_double_safe = false;
                requires_quote = true;
            }
            if (!requires_quote || is_single_safe) && unicode::is_single_quote(ch) {
                is_single_safe = false;
                requires_quote = true;
            }
            if crate::is_bidi(ch) {
                is_bidi = true;
            }
            if crate::requires_escape(ch) {
                return write_escaped(f, text.chars().map(Ok));
            }
        }
    }

    if is_bidi && crate::is_suspicious_bidi(text) {
        return write_escaped(f, text.chars().map(Ok));
    }

    if !requires_quote {
        f.write_str(text)
    } else if is_single_safe {
        write_simple(f, text, '\'')
    } else if is_double_safe {
        write_simple(f, text, '\"')
    } else {
        write_single_escaped(f, text)
    }
}

fn write_simple(f: &mut Formatter<'_>, text: &str, quote: char) -> fmt::Result {
    f.write_char(quote)?;
    f.write_str(text)?;
    f.write_char(quote)?;
    Ok(())
}

fn write_single_escaped(f: &mut Formatter<'_>, text: &str) -> fmt::Result {
    // Quotes in PowerShell are escaped by doubling them.
    // The second quote is used, so '‘ becomes ‘.
    // Therefore we insert a ' before every quote we find.

    // If we think something is a single quote and quote it but the PowerShell
    // version doesn't (e.g. because it's old) then things go wrong. I don't
    // know of a way to solve this. A ` (backtick) escape only works between
    // double quotes or in a bare string. We can't unquote, use a bare string,
    // then requote, as we would in Unix: PowerShell sees that as multiple
    // arguments.
    f.write_char('\'')?;
    let mut pos = 0;
    for (index, _) in text.match_indices(unicode::is_single_quote) {
        f.write_str(&text[pos..index])?;
        f.write_char('\'')?;
        pos = index;
    }
    f.write_str(&text[pos..])?;
    f.write_char('\'')?;
    Ok(())
}

pub(crate) fn write_escaped(
    f: &mut Formatter<'_>,
    text: impl Iterator<Item = Result<char, u16>>,
) -> fmt::Result {
    // ` takes the role of \ since \ is already used as the path separator.
    // Things are UTF-16-oriented, so we escape bad code units as "`u{1234}".

    f.write_char('"')?;
    for ch in text {
        match ch {
            Ok(ch) => match ch {
                '\0' => f.write_str("`0")?,
                '\r' => f.write_str("`r")?,
                '\n' => f.write_str("`n")?,
                '\t' => f.write_str("`t")?,
                // Code unit escapes are only supported in PowerShell Core,
                // so we're more willing to use weird escapes here than on Unix.
                // There's also `e, for \x1B, but that one's Core-exclusive.
                '\x07' => f.write_str("`a")?,
                '\x08' => f.write_str("`b")?,
                '\x0b' => f.write_str("`v")?,
                '\x0c' => f.write_str("`f")?,
                ch if crate::requires_escape(ch) || crate::is_bidi(ch) => {
                    write!(f, "`u{{{:02X}}}", ch as u32)?
                }
                '`' => f.write_str("``")?,
                '$' => f.write_str("`$")?,
                ch if unicode::is_double_quote(ch) => {
                    // We can quote this with either ` or ".
                    // But if we use " and the PowerShell version doesn't actually
                    // see this as a double quote then we're in trouble.
                    // ` is safer.
                    f.write_char('`')?;
                    f.write_char(ch)?;
                }
                ch => f.write_char(ch)?,
            },
            Err(unit) => write!(f, "`u{{{:04X}}}", unit)?,
        }
    }
    f.write_char('"')?;
    Ok(())
}

/// PowerShell makes liberal use of Unicode:
/// <https://github.com/PowerShell/PowerShell/blob/master/src/System.Management.Automation/engine/parser/CharTraits.cs>
/// This may have to be updated in the future.
mod unicode {
    /// PowerShell considers these to be whitespace:
    /// 1. ASCII: Space, Horizontal tab, Form feed, Carriage return
    /// 2. Unicode: No-break space, Next line
    /// 3. Everything that satisfies System.Char.IsSeparator, i.e. everything
    ///    in the categories {space, line, paragraph} separator
    ///
    /// This overlaps with but is not identical to char::is_whitespace().
    ///
    /// There is some redundancy throughout this implementation. We already
    /// know that ch is not ASCII, and \u{A0} is repeated. But that's all
    /// optimized away in the end so no need to worry about it.
    pub(crate) fn is_whitespace(ch: char) -> bool {
        match ch {
            ' ' | '\t' | '\x0B' | '\x0C' => true,
            '\u{00A0}' | '\u{0085}' => true,
            c => is_separator(c),
        }
    }

    /// I don't want to add a dependency just for this, and
    /// as of writing, the unicode_categories crate is out of
    /// date anyway. So hardcode the category check.
    ///
    /// curl -s https://www.unicode.org/Public/UCD/latest/ucd/UnicodeData.txt \
    ///     | grep -e Zl -e Zp -e Zs | cut -d ';' -f 1
    ///
    /// Unicode 15.0 will release on September 11, 2022.
    fn is_separator(ch: char) -> bool {
        match ch {
            '\u{0020}' | '\u{00A0}' | '\u{1680}' | '\u{2000}' | '\u{2001}' | '\u{2002}'
            | '\u{2003}' | '\u{2004}' | '\u{2005}' | '\u{2006}' | '\u{2007}' | '\u{2008}'
            | '\u{2009}' | '\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}'
            | '\u{3000}' => true,
            _ => false,
        }
    }

    /// These can be used to start options.
    ///
    /// There exist others, but PowerShell doesn't care about them.
    pub(crate) fn is_dash(ch: char) -> bool {
        match ch {
            '-' | '\u{2013}' | '\u{2014}' | '\u{2015}' => true,
            _ => false,
        }
    }

    pub(crate) fn is_single_quote(ch: char) -> bool {
        match ch {
            '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => true,
            _ => false,
        }
    }

    pub(crate) fn is_double_quote(ch: char) -> bool {
        match ch {
            '"' | '\u{201C}' | '\u{201D}' | '\u{201E}' => true,
            _ => false,
        }
    }
}
