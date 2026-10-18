# On Margins

The best interfaces **disappear**. A document should read like a page, not a form — its controls living quietly at the edges, arriving only when you reach for them.[^1]

> “Write as if it were print.”

- [x] Calm by default
- [ ] Controls grow from content
- Syntax lives in the *margin*, not the text

## Code

```rust
fn main() {
    render(&doc);
}
```

1. Inline `code`, ~~strikethrough~~, and [links](https://example.com)
2. Entities like &amp; and escapes like \*these\*

| Element | Glyph | Control       |
|---------|-------|---------------|
| Heading | #     | Level by drag |
| Table   | \|    | Edge handles  |

---

[^1]: Dieter Rams, *Less but better*, 1995.
