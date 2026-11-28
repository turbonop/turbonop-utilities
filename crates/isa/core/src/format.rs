//! Text: classified tokens, and names the caller supplies.

/// What a piece of formatted text is (for coloring, and for tools that
/// read it back).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TokenKind {
    /// A prefix (`lock`, `rep`).
    Prefix,
    /// The mnemonic.
    Mnemonic,
    /// A register.
    Register,
    /// A number: an immediate, a displacement, an index.
    Number,
    /// An address (a branch's target, memory's).
    Address,
    /// A name a [`Resolver`] supplied (of an address, an index, a token).
    Name,
    /// A keyword (`byte ptr`, `lsl`, a condition).
    Keyword,
    /// A decoration (`{k1}`, `{z}`, `{rn-sae}`, `!`).
    Decorator,
    /// Punctuation and spacing.
    Punctuation,
    /// A comment.
    Comment,
    /// Text not classified further (by a formatter that doesn't yet say
    /// what its pieces are).
    Text,
}

/// Receives formatted text, token by token.
pub trait TokenSink {
    /// Takes the next piece of text, and what it is.
    fn token(&mut self, kind: TokenKind, text: &str);
}

impl TokenSink for alloc::string::String {
    #[inline]
    fn token(&mut self, _: TokenKind, text: &str) {
        self.push_str(text);
    }
}

impl TokenSink for core::fmt::Formatter<'_> {
    #[inline]
    fn token(&mut self, _: TokenKind, text: &str) {
        let _ = self.write_str(text);
    }
}

/// Which kind of entity an index names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Space {
    /// A function.
    Function,
    /// A type (a signature, a class).
    Type,
    /// A global.
    Global,
    /// A table.
    Table,
    /// A memory.
    Memory,
    /// A local.
    Local,
    /// An argument.
    Argument,
    /// A data segment.
    Data,
    /// An element segment.
    Element,
    /// An exception tag.
    Tag,
    /// A field.
    Field,
    /// A string.
    String,
    /// A constant pool's entry.
    Constant,
}

/// What an operand names, for a [`Resolver`] to name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Ref {
    /// An address.
    Address(u64),
    /// An index into a space (WebAssembly's, a constant pool's).
    Index(Space, u32),
    /// A metadata token (.NET's: its table in its top byte).
    Token(u32),
}

/// Names what operands refer to: symbols for addresses, names for indices
/// and tokens. What it doesn't name, the formatter writes as a number.
pub trait Resolver {
    /// Writes `r`'s name to `out` (as [`TokenKind::Name`] tokens, and
    /// others if it likes), and returns `true`; or writes nothing and
    /// returns `false`.
    fn resolve(&self, r: Ref, out: &mut dyn TokenSink) -> bool;
}

/// Names nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Unresolved;

impl Resolver for Unresolved {
    #[inline]
    fn resolve(&self, _: Ref, _: &mut dyn TokenSink) -> bool {
        false
    }
}
