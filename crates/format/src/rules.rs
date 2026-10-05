//! What goes between two tokens: a space, nothing, a line break, or what the code has. Only
//! whitespace is decided here, so no rule can change what a program means.

use php_syntax::SyntaxKind;
use php_syntax::SyntaxKind::*;

use crate::model::Model;
use crate::options::{BraceStyle, FormatOptions};
use crate::wrap::{Forced, Wrap, broken_before};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rule {
    /// One space, or the break the code has.
    Space,
    /// Nothing, or the break the code has.
    NoSpace,
    /// What the code has.
    Keep,
    /// A break, with the blank lines the code has.
    Break,
    /// A break and no blank line.
    BreakTight,
    /// A break and exactly this many blank lines.
    BreakBlank(usize),
    /// One space and no break.
    Join,
    /// Nothing and no break.
    JoinTight,
}

/// What a brace opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Braces {
    Class,
    AnonymousClass,
    Function,
    Closure,
    Control,
    Standalone,
    Switch,
    Other,
    /// A brace with nothing inside it set off: a group of names, a dynamic member name.
    Tight,
}

pub(crate) fn braces(m: &Model, position: usize) -> Braces {
    let (parent, grand) = (m.parent(position), m.grandparent(position));
    match parent {
        CLASS_BODY if grand == ANONYMOUS_CLASS => Braces::AnonymousClass,
        CLASS_BODY => Braces::Class,
        BLOCK => match grand {
            FUNCTION_DECLARATION | METHOD_DECLARATION => Braces::Function,
            CLOSURE_EXPR | PROPERTY_HOOK => Braces::Closure,
            IF_STATEMENT
            | ELSEIF_CLAUSE
            | ELSE_CLAUSE
            | WHILE_STATEMENT
            | DO_WHILE_STATEMENT
            | FOR_STATEMENT
            | FOREACH_STATEMENT
            | TRY_STATEMENT
            | CATCH_CLAUSE
            | FINALLY_CLAUSE
            | DECLARE_STATEMENT
            | NAMESPACE_DECLARATION => Braces::Control,
            BLOCK | STATEMENT_LIST | SOURCE_FILE | CASE_CLAUSE | DEFAULT_CLAUSE => Braces::Standalone,
            _ => Braces::Other,
        },
        SWITCH_STATEMENT => Braces::Switch,
        USE_GROUP | PROPERTY_FETCH_EXPR | SCOPED_ACCESS_EXPR | STATIC_PROPERTY_EXPR | VARIABLE_VARIABLE => {
            Braces::Tight
        }
        _ => Braces::Other,
    }
}

/// Whether a token ends a value, so that something like `(` or `[` after it is a call or an index.
fn ends_value(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        IDENT
            | QUALIFIED_NAME
            | FULLY_QUALIFIED_NAME
            | RELATIVE_NAME
            | VARIABLE
            | RPAREN
            | RBRACKET
            | RBRACE
            | STRING_LITERAL
            | INT_LITERAL
            | FLOAT_LITERAL
            | MAGIC_CONSTANT
            | DOUBLE_QUOTE
            | BACKTICK
            | HEREDOC_END
    )
}

/// A token that stands for a word or a value, which another one cannot touch.
fn is_word(kind: SyntaxKind) -> bool {
    ends_value(kind) && !matches!(kind, RPAREN | RBRACKET | RBRACE)
        || kind.is_keyword()
        || matches!(kind, HEREDOC_START | CAST)
}

fn is_binary_operator(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        PLUS | MINUS
            | STAR
            | SLASH
            | PERCENT
            | POW
            | DOT
            | EQ
            | NEQ
            | IDENTICAL
            | NOT_IDENTICAL
            | LT
            | GT
            | LE
            | GE
            | SPACESHIP
            | AND_AND
            | OR_OR
            | AMP
            | PIPE
            | CARET
            | SHL
            | SHR
            | COALESCE
            | PIPE_GT
            | AND_KW
            | OR_KW
            | XOR_KW
            | INSTANCEOF_KW
    )
}

fn is_assignment(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        ASSIGN
            | PLUS_ASSIGN
            | MINUS_ASSIGN
            | STAR_ASSIGN
            | SLASH_ASSIGN
            | DOT_ASSIGN
            | PERCENT_ASSIGN
            | POW_ASSIGN
            | COALESCE_ASSIGN
            | AMP_ASSIGN
            | PIPE_ASSIGN
            | CARET_ASSIGN
            | SHL_ASSIGN
            | SHR_ASSIGN
    )
}

fn is_prefix_operator(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        BANG | TILDE | AT | MINUS | PLUS | INC | DEC | AMP | ELLIPSIS | DOLLAR
    )
}

/// The keywords that open a parenthesis of their own, which a space tells from a call.
fn is_control_keyword(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        IF_KW | ELSEIF_KW | WHILE_KW | FOR_KW | FOREACH_KW | SWITCH_KW | CATCH_KW | MATCH_KW
    )
}

/// The start of the argument or parameter list a token begins an item of, or closes.
fn list_broken_before(m: &Model, forced: &Forced, n: usize) -> bool {
    if forced.is_empty() {
        return false;
    }
    let list_start = |node: &php_syntax::SyntaxNode| usize::from(node.text_range().start());
    if m.kind(n) == RPAREN && matches!(m.parent(n), ARGUMENT_LIST | PARAMETER_LIST) {
        return forced.contains(&(Wrap::List, list_start(&m.leaf(n).parent)));
    }
    m.starts[n].iter().any(|node| {
        matches!(node.kind(), ARGUMENT | PARAMETER)
            && node
                .parent()
                .is_some_and(|list| forced.contains(&(Wrap::List, list_start(&list))))
    })
}

/// Whether the brace at `n` is followed at once by its closing brace, with nothing between them.
fn is_empty_body(m: &Model, n: usize) -> bool {
    n + 1 < m.sig.len()
        && m.kind(n + 1) == RBRACE
        && m.leaves[m.sig[n] + 1..m.sig[n + 1]]
            .iter()
            .all(|leaf| leaf.kind == WHITESPACE)
}

/// Whether the parameters of the function a brace opens run over more than one line.
fn multiline_parameters(m: &Model, forced: &Forced, n: usize) -> bool {
    let Some(function) = m.leaf(n).parent.parent() else {
        return false;
    };
    function
        .children()
        .find(|child| child.kind() == PARAMETER_LIST)
        .is_some_and(|list| {
            forced.contains(&(Wrap::List, usize::from(list.text_range().start())))
                || list.text().to_string().contains('\n')
        })
}

pub(crate) fn rule(m: &Model, options: &FormatOptions, forced: &Forced, p: usize, n: usize) -> Rule {
    let (pl, nl) = (m.leaf(p), m.leaf(n));
    if let (Some(left), Some(right)) = (pl.opaque, nl.opaque) {
        if left == right {
            return Rule::Keep;
        }
    }
    let (pk, nk) = (m.kind(p), m.kind(n));
    let (pp, np) = (m.parent(p), m.parent(n));

    if pk == HALT_DATA || nk == HALT_DATA {
        return Rule::Keep;
    }
    if pk == OPEN_TAG {
        return Rule::Keep;
    }
    if nk == CLOSE_TAG {
        return Rule::Break;
    }
    if list_broken_before(m, forced, n) || broken_before(m, forced, n) {
        return Rule::BreakTight;
    }
    if let Some(boundary) = m.boundaries[n] {
        return boundary_rule(options, boundary.kind, boundary.previous);
    }

    if nk == COMMA {
        return Rule::NoSpace;
    }
    if nk == SEMICOLON {
        return Rule::NoSpace;
    }
    if pk == SEMICOLON && pp == FOR_STATEMENT {
        return if matches!(nk, RPAREN | SEMICOLON) {
            Rule::NoSpace
        } else {
            Rule::Space
        };
    }
    if pk == COMMA {
        return if matches!(nk, RPAREN | RBRACKET) {
            Rule::NoSpace
        } else {
            Rule::Space
        };
    }

    if matches!(nk, LBRACE) {
        return brace_before(m, options, forced, p, n);
    }
    if matches!(pk, LBRACE) {
        return brace_after(m, n, p);
    }
    if matches!(nk, RBRACE) {
        return brace_before_close(m, n, p);
    }
    if matches!(pk, RBRACE) {
        if matches!(nk, ELSE_KW | ELSEIF_KW | CATCH_KW | FINALLY_KW) || nk == WHILE_KW && np == DO_WHILE_STATEMENT {
            return Rule::Join;
        }
        if matches!(nk, ARROW | NULLSAFE_ARROW | DOUBLE_COLON) {
            return Rule::NoSpace;
        }
    }

    if matches!(pk, LPAREN | LBRACKET | HASH_BRACKET) || matches!(nk, RPAREN | RBRACKET) {
        return Rule::NoSpace;
    }
    if pk == CAST {
        return Rule::Keep;
    }
    if nk == LPAREN {
        return paren_before(m, p, n);
    }
    if nk == LBRACKET {
        return if np == INDEX_EXPR || ends_value(pk) && !pk.is_keyword() {
            Rule::NoSpace
        } else {
            Rule::Space
        };
    }
    if nk == HASH_BRACKET {
        return Rule::Space;
    }

    if matches!(pk, ARROW | NULLSAFE_ARROW | DOUBLE_COLON | BACKSLASH)
        || matches!(nk, ARROW | NULLSAFE_ARROW | DOUBLE_COLON | BACKSLASH)
    {
        return Rule::NoSpace;
    }

    if nk == COLON {
        return match np {
            TERNARY_EXPR if pk == QUESTION => Rule::NoSpace,
            TERNARY_EXPR => Rule::Space,
            _ => Rule::NoSpace,
        };
    }
    if pk == COLON {
        return Rule::Space;
    }
    if nk == QUESTION && np == TERNARY_EXPR {
        return Rule::Space;
    }
    if pk == QUESTION {
        return if pp == TERNARY_EXPR && nk != COLON {
            Rule::Space
        } else {
            Rule::NoSpace
        };
    }

    if nk == FAT_ARROW || pk == FAT_ARROW {
        return Rule::Space;
    }
    if is_assignment(nk) || is_assignment(pk) {
        return if np == DECLARE_DIRECTIVE || pp == DECLARE_DIRECTIVE {
            Rule::NoSpace
        } else {
            Rule::Space
        };
    }

    if (nk == INC || nk == DEC) && np == POSTFIX_EXPR {
        return Rule::NoSpace;
    }
    if pp == PREFIX_EXPR {
        return Rule::NoSpace;
    }
    if matches!(pk, ELLIPSIS | DOLLAR) {
        return Rule::NoSpace;
    }
    if nk == ELLIPSIS {
        return Rule::Space;
    }
    if pk == RBRACKET && pp == ATTRIBUTE_LIST {
        return Rule::Space;
    }
    if nk == QUESTION && np == NULLABLE_TYPE && is_word(pk) {
        return Rule::Space;
    }
    if pk == CAST {
        return Rule::Keep;
    }
    if pk == AMP && pp != BINARY_EXPR {
        return Rule::NoSpace;
    }
    if matches!(pk, QUESTION) && pp == NULLABLE_TYPE {
        return Rule::NoSpace;
    }

    if matches!(nk, PIPE | AMP) && matches!(np, UNION_TYPE | INTERSECTION_TYPE)
        || matches!(pk, PIPE | AMP) && matches!(pp, UNION_TYPE | INTERSECTION_TYPE)
    {
        return Rule::NoSpace;
    }
    if nk == AMP && np != BINARY_EXPR {
        return Rule::Space;
    }
    if is_binary_operator(nk) && np == BINARY_EXPR || is_binary_operator(pk) && pp == BINARY_EXPR {
        return Rule::Space;
    }
    if matches!(nk, AS_KW | INSTANCEOF_KW | INSTEADOF_KW | EXTENDS_KW | IMPLEMENTS_KW)
        || matches!(pk, AS_KW | INSTEADOF_KW)
    {
        return Rule::Space;
    }

    if is_word(pk)
        && (is_word(nk)
            || is_prefix_operator(nk) && np == PREFIX_EXPR
            || matches!(nk, DOUBLE_QUOTE | BACKTICK | HEREDOC_START | CAST | LBRACKET))
    {
        return Rule::Space;
    }
    if is_word(nk) && matches!(pk, RPAREN | RBRACKET | RBRACE) {
        return Rule::Space;
    }
    Rule::Keep
}

fn boundary_rule(options: &FormatOptions, kind: SyntaxKind, previous: SyntaxKind) -> Rule {
    if kind == METHOD_DECLARATION || previous == METHOD_DECLARATION {
        return Rule::BreakBlank(options.blank_lines_between_members);
    }
    let declaration = |kind| {
        matches!(
            kind,
            FUNCTION_DECLARATION | CLASS_DECLARATION | INTERFACE_DECLARATION | TRAIT_DECLARATION | ENUM_DECLARATION
        )
    };
    if declaration(kind) || declaration(previous) {
        return Rule::BreakBlank(1);
    }
    let header = |kind| matches!(kind, NAMESPACE_DECLARATION | DECLARE_STATEMENT);
    if header(previous) || kind == NAMESPACE_DECLARATION {
        return Rule::BreakBlank(1);
    }
    if (kind == USE_STATEMENT) != (previous == USE_STATEMENT) {
        return Rule::BreakBlank(1);
    }
    Rule::Break
}

fn brace_before(m: &Model, options: &FormatOptions, forced: &Forced, p: usize, n: usize) -> Rule {
    let own_line = |style: BraceStyle| match style {
        BraceStyle::NextLine => Rule::BreakTight,
        BraceStyle::SameLine => Rule::Join,
    };
    match braces(m, n) {
        Braces::Class | Braces::Function if is_empty_body(m, n) => Rule::Join,
        Braces::Class => own_line(options.class_brace),
        Braces::Function if multiline_parameters(m, forced, n) => Rule::Join,
        Braces::Function => own_line(options.function_brace),
        Braces::Tight => Rule::JoinTight,
        Braces::Standalone => Rule::Break,
        Braces::AnonymousClass | Braces::Closure | Braces::Control | Braces::Switch | Braces::Other => {
            if m.kind(p) == BACKSLASH {
                Rule::JoinTight
            } else if matches!(m.kind(p), ARROW | NULLSAFE_ARROW | DOUBLE_COLON | DOLLAR) {
                Rule::NoSpace
            } else {
                Rule::Join
            }
        }
    }
}

fn brace_after(m: &Model, n: usize, p: usize) -> Rule {
    if m.kind(n) == RBRACE {
        return Rule::JoinTight;
    }
    match braces(m, p) {
        Braces::Class | Braces::AnonymousClass => Rule::Break,
        Braces::Function | Braces::Control | Braces::Standalone | Braces::Switch => Rule::BreakTight,
        Braces::Tight => Rule::NoSpace,
        Braces::Closure | Braces::Other => Rule::Space,
    }
}

fn brace_before_close(m: &Model, n: usize, p: usize) -> Rule {
    if m.kind(p) == LBRACE {
        return Rule::JoinTight;
    }
    match braces(m, n) {
        Braces::Class | Braces::AnonymousClass => Rule::Break,
        Braces::Function | Braces::Control | Braces::Standalone | Braces::Switch => Rule::BreakTight,
        Braces::Tight => Rule::NoSpace,
        Braces::Closure | Braces::Other => Rule::Space,
    }
}

fn paren_before(m: &Model, p: usize, n: usize) -> Rule {
    let (pk, np) = (m.kind(p), m.parent(n));
    match np {
        ARGUMENT_LIST | MODIFIER_LIST => Rule::NoSpace,
        PARAMETER_LIST => {
            if pk == FUNCTION_KW && m.parent(p) == CLOSURE_EXPR {
                Rule::Space
            } else {
                Rule::NoSpace
            }
        }
        CLOSURE_USE => Rule::Space,
        IF_STATEMENT | ELSEIF_CLAUSE | WHILE_STATEMENT | DO_WHILE_STATEMENT | FOR_STATEMENT | FOREACH_STATEMENT
        | SWITCH_STATEMENT | CATCH_CLAUSE | MATCH_EXPR => {
            if is_control_keyword(pk) {
                Rule::Space
            } else {
                Rule::NoSpace
            }
        }
        DECLARE_STATEMENT | ISSET_EXPR | EMPTY_EXPR | EXIT_EXPR | LIST_EXPR | UNSET_STATEMENT | EVAL_EXPR
        | ARRAY_EXPR | PAREN_TYPE => Rule::NoSpace,
        PAREN_EXPR => {
            let binary = m.parent(p) == BINARY_EXPR && is_binary_operator(pk);
            if !binary
                && matches!(
                    pk,
                    LPAREN | LBRACKET | BANG | AT | TILDE | INC | DEC | MINUS | PLUS | AMP | PIPE | ELLIPSIS
                )
            {
                Rule::NoSpace
            } else {
                Rule::Space
            }
        }
        _ => {
            if is_word(pk) && !pk.is_keyword() || matches!(pk, RPAREN | RBRACKET | FN_KW | STATIC_KW) {
                Rule::NoSpace
            } else {
                Rule::Space
            }
        }
    }
}
