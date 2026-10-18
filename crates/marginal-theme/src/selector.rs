//! CSS selectors over a Markdown document's structure: compound selectors (`li.task`,
//! `td:first-child`), descendant and child combinators (`blockquote p`, `li > p`), structural
//! pseudo-classes and `::before`/`::after`. Sibling combinators, attribute selectors and
//! interactive states (`:hover`) aren't supported; those selectors are reported and skipped.

/// An element in the document and where it sits among its siblings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Node {
    pub element: String,
    pub classes: Vec<String>,
    /// Position among siblings, from 0.
    pub index: usize,
    /// Number of siblings including this one; 0 when unknown (then `:last-child` never matches).
    pub count: usize,
}

impl Node {
    pub fn new(element: &str) -> Self {
        Self { element: element.into(), classes: Vec::new(), index: 0, count: 1 }
    }

    pub fn with_classes(mut self, classes: &[&str]) -> Self {
        self.classes = classes.iter().map(|c| (*c).to_owned()).collect();
        self
    }

    pub fn at(mut self, index: usize, count: usize) -> Self {
        self.index = index;
        self.count = count;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PseudoElement {
    Before,
    After,
}

#[derive(Clone, Debug, PartialEq)]
enum Pseudo {
    FirstChild,
    LastChild,
    OnlyChild,
    /// `:nth-child(an+b)`, and `:nth-last-child` with `from_end`.
    Nth {
        a: i32,
        b: i32,
        from_end: bool,
    },
    Not(Box<Compound>),
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Compound {
    /// `None` matches any element (`*` or no type).
    element: Option<String>,
    classes: Vec<String>,
    ids: usize,
    pseudos: Vec<Pseudo>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Combinator {
    Descendant,
    Child,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Selector {
    /// Compounds left to right; `combinators[i]` sits between `compounds[i]` and `[i + 1]`.
    compounds: Vec<Compound>,
    combinators: Vec<Combinator>,
    pub pseudo_element: Option<PseudoElement>,
}

/// Typora's own names for the same elements.
fn normalize(text: &str) -> String {
    text.replace(".md-fences", "pre").replace("pre.pre", "pre").replace(".md-task-list-item", "li.task")
}

impl Selector {
    pub fn parse(text: &str) -> Option<Self> {
        let text = normalize(text);
        let mut compounds = Vec::new();
        let mut combinators = Vec::new();
        let mut pseudo_element = None;
        let mut pending: Option<Combinator> = None;
        let mut chars = text.chars().peekable();
        let mut current = String::new();
        let mut depth = 0;
        let flush = |current: &mut String,
                     compounds: &mut Vec<Compound>,
                     combinators: &mut Vec<Combinator>,
                     pending: &mut Option<Combinator>,
                     pseudo_element: &mut Option<PseudoElement>|
         -> Option<()> {
            if current.is_empty() {
                return Some(());
            }
            if pseudo_element.is_some() {
                return None; // a pseudo-element must end the selector
            }
            let (compound, pe) = parse_compound(current)?;
            *pseudo_element = pe;
            if !compounds.is_empty() {
                combinators.push(pending.take().unwrap_or(Combinator::Descendant));
            }
            compounds.push(compound);
            current.clear();
            Some(())
        };
        while let Some(c) = chars.next() {
            match c {
                '(' => {
                    depth += 1;
                    current.push(c);
                }
                ')' => {
                    depth -= 1;
                    current.push(c);
                }
                ' ' | '>' | '+' | '~' if depth == 0 => {
                    flush(&mut current, &mut compounds, &mut combinators, &mut pending, &mut pseudo_element)?;
                    match c {
                        '>' => pending = Some(Combinator::Child),
                        '+' | '~' => return None,
                        _ => {
                            if pending.is_none() && !compounds.is_empty() {
                                // Whitespace: a descendant combinator unless `>` follows.
                                while chars.peek() == Some(&' ') {
                                    chars.next();
                                }
                                if matches!(chars.peek(), Some('>' | '+' | '~')) {
                                    continue;
                                }
                                pending = Some(Combinator::Descendant);
                            }
                        }
                    }
                }
                '[' => return None,
                c => current.push(c),
            }
        }
        flush(&mut current, &mut compounds, &mut combinators, &mut pending, &mut pseudo_element)?;
        if compounds.is_empty() || pending.is_some() {
            return None;
        }
        Some(Self { compounds, combinators, pseudo_element })
    }

    /// (ids, classes and pseudo-classes, elements), as in CSS.
    pub fn specificity(&self) -> (usize, usize, usize) {
        let mut s = (0, 0, 0);
        for c in &self.compounds {
            s.0 += c.ids;
            s.1 += c.classes.len() + c.pseudos.len();
            s.2 += usize::from(c.element.is_some() && c.ids == 0);
        }
        s.2 += usize::from(self.pseudo_element.is_some());
        s
    }

    /// A lone `body`/`#write`/`:root` with nothing else: where theme variables live.
    pub fn is_root(&self) -> bool {
        matches!(self.compounds.as_slice(), [c] if c.element.as_deref() == Some("body") && c.classes.is_empty() && c.pseudos.is_empty())
            && self.pseudo_element.is_none()
    }

    /// Whether the selector matches the last node of `path` (the root first).
    pub fn matches(&self, path: &[Node]) -> bool {
        let Some((subject, ancestors)) = path.split_last() else { return false };
        let Some((last, rest)) = self.compounds.split_last() else { return false };
        if !last.matches(subject) {
            return false;
        }
        matches_rest(rest, &self.combinators, ancestors)
    }
}

/// Matches compounds (right to left) against ancestors (nearest last).
fn matches_rest(compounds: &[Compound], combinators: &[Combinator], ancestors: &[Node]) -> bool {
    let Some((compound, rest)) = compounds.split_last() else { return true };
    let combinator = combinators[compounds.len() - 1];
    match combinator {
        Combinator::Child => match ancestors.split_last() {
            Some((parent, above)) => {
                compound.matches(parent) && matches_rest(rest, &combinators[..compounds.len() - 1], above)
            }
            None => false,
        },
        Combinator::Descendant => (0..ancestors.len()).rev().any(|i| {
            compound.matches(&ancestors[i]) && matches_rest(rest, &combinators[..compounds.len() - 1], &ancestors[..i])
        }),
    }
}

impl Compound {
    fn matches(&self, node: &Node) -> bool {
        self.element.as_deref().is_none_or(|e| e == node.element)
            && self.classes.iter().all(|c| node.classes.contains(c))
            && self.pseudos.iter().all(|p| match p {
                Pseudo::FirstChild => node.index == 0,
                Pseudo::LastChild => node.count > 0 && node.index + 1 == node.count,
                Pseudo::OnlyChild => node.count == 1,
                Pseudo::Nth { a, b, from_end } => {
                    if *from_end && node.count == 0 {
                        return false;
                    }
                    let pos = if *from_end { node.count - node.index } else { node.index + 1 } as i32;
                    match a {
                        0 => pos == *b,
                        a => (pos - b) % a == 0 && (pos - b) / a >= 0,
                    }
                }
                Pseudo::Not(inner) => !inner.matches(node),
            })
    }
}

/// `an+b`, `odd`, `even`, `3`.
fn parse_nth(arg: &str) -> Option<(i32, i32)> {
    let s: String = arg.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_lowercase();
    match s.as_str() {
        "odd" => return Some((2, 1)),
        "even" => return Some((2, 0)),
        _ => {}
    }
    match s.split_once('n') {
        Some((a, b)) => {
            let a = match a {
                "" | "+" => 1,
                "-" => -1,
                a => a.parse().ok()?,
            };
            let b = if b.is_empty() { 0 } else { b.parse().ok()? };
            Some((a, b))
        }
        None => Some((0, s.parse().ok()?)),
    }
}

/// Parses one compound like `li.task:first-child::before`.
fn parse_compound(text: &str) -> Option<(Compound, Option<PseudoElement>)> {
    let mut compound = Compound::default();
    let mut pseudo_element = None;
    let mut rest = text;
    let ident_end = |s: &str| s.find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_')).unwrap_or(s.len());
    // The type selector.
    let n = if rest.starts_with('*') { 1 } else { ident_end(rest) };
    if n > 0 {
        let name = &rest[..n];
        compound.element = match name {
            "*" => None,
            "html" | "body" => Some("body".into()),
            other => Some(other.to_lowercase()),
        };
        rest = &rest[n..];
    }
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("::") {
            let n = ident_end(r);
            pseudo_element = Some(match &r[..n] {
                "before" => PseudoElement::Before,
                "after" => PseudoElement::After,
                _ => return None,
            });
            rest = &r[n..];
        } else if let Some(r) = rest.strip_prefix('.') {
            let n = ident_end(r);
            if n == 0 {
                return None;
            }
            compound.classes.push(r[..n].to_owned());
            rest = &r[n..];
        } else if let Some(r) = rest.strip_prefix('#') {
            let n = ident_end(r);
            match &r[..n] {
                // Typora's editor root is the document.
                "write" => {
                    if compound.element.is_none() {
                        compound.element = Some("body".into());
                    }
                    compound.ids += 1;
                }
                _ => return None,
            }
            rest = &r[n..];
        } else {
            let r = rest.strip_prefix(':')?;
            let n = ident_end(r);
            let name = &r[..n];
            let after = &r[n..];
            let (arg, after) = match after.strip_prefix('(') {
                Some(a) => {
                    let close = a.find(')')?;
                    (Some(&a[..close]), &a[close + 1..])
                }
                None => (None, after),
            };
            match (name, arg) {
                ("root", None) => compound.element = Some("body".into()),
                ("before", None) => pseudo_element = Some(PseudoElement::Before),
                ("after", None) => pseudo_element = Some(PseudoElement::After),
                ("first-child" | "first-of-type", None) => compound.pseudos.push(Pseudo::FirstChild),
                ("last-child" | "last-of-type", None) => compound.pseudos.push(Pseudo::LastChild),
                ("only-child" | "only-of-type", None) => compound.pseudos.push(Pseudo::OnlyChild),
                ("nth-child" | "nth-of-type", Some(arg)) => {
                    let (a, b) = parse_nth(arg)?;
                    compound.pseudos.push(Pseudo::Nth { a, b, from_end: false });
                }
                ("nth-last-child" | "nth-last-of-type", Some(arg)) => {
                    let (a, b) = parse_nth(arg)?;
                    compound.pseudos.push(Pseudo::Nth { a, b, from_end: true });
                }
                ("not", Some(arg)) => {
                    let (inner, pe) = parse_compound(arg.trim())?;
                    if pe.is_some() {
                        return None;
                    }
                    compound.pseudos.push(Pseudo::Not(Box::new(inner)));
                }
                _ => return None,
            }
            rest = after;
        }
    }
    Some((compound, pseudo_element))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(nodes: &[Node]) -> Vec<Node> {
        nodes.to_vec()
    }

    #[test]
    fn descendants_children_and_positions() {
        let body = Node::new("body");
        let quote = Node::new("blockquote");
        let p = Node::new("p").at(1, 3);
        let in_quote = path(&[body.clone(), quote.clone(), p.clone()]);
        let top = path(&[body.clone(), p.clone()]);
        let s = |t: &str| Selector::parse(t).unwrap_or_else(|| panic!("{t} parses"));
        assert!(s("#write blockquote p").matches(&in_quote));
        assert!(!s("#write blockquote p").matches(&top));
        assert!(s("blockquote > p").matches(&in_quote));
        assert!(!s("body > p:first-child").matches(&top));
        assert!(s("p:nth-child(2)").matches(&top));
        assert!(s("p:nth-child(2n)").matches(&top));
        assert!(!s("p:nth-child(odd)").matches(&top));
        assert!(s("p:not(.x)").matches(&top));
        assert!(s("p:nth-last-child(2)").matches(&top));
        let tr =
            path(&[body.clone(), Node::new("table"), Node::new("tbody"), Node::new("tr").at(1, 4), Node::new("td")]);
        assert!(s("table tr:nth-child(2n) td").matches(&tr));
        assert!(s("tbody>tr:nth-child(even)>td").matches(&tr));
    }

    #[test]
    fn specificity_and_unsupported() {
        let s = |t: &str| Selector::parse(t).unwrap();
        assert!(s("#write h1").specificity() > s("h1.x").specificity());
        assert!(s("h1.x").specificity() > s("h1").specificity());
        assert_eq!(s("h1::before").pseudo_element, Some(PseudoElement::Before));
        assert_eq!(s("h2:after").pseudo_element, Some(PseudoElement::After));
        assert!(s("#write").is_root() && s(":root").is_root() && !s("#write > p").is_root());
        for bad in ["h1 + p", "a[href]", "a:hover", "#sidebar p", "h1::before span", "p >"] {
            assert!(Selector::parse(bad).is_none(), "{bad} is unsupported");
        }
    }
}
