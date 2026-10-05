//! Every token and node kind of the syntax tree, in one enum so rowan can store it as a `u16`.

macro_rules! syntax_kinds {
    ($($name:ident),* $(,)?) => {
        /// The kind of a token or a node.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[repr(u16)]
        #[allow(non_camel_case_types, clippy::upper_case_acronyms)]
        pub enum SyntaxKind {
            $($name),*
        }

        impl SyntaxKind {
            /// Every kind in declaration order; the index is the raw value.
            pub const ALL: &'static [SyntaxKind] = &[$(SyntaxKind::$name),*];

            /// The name of the variant, as the tree dump prints it.
            pub fn name(self) -> &'static str {
                match self {
                    $(SyntaxKind::$name => stringify!($name)),*
                }
            }
        }
    };
}

syntax_kinds! {
    // Trivia
    WHITESPACE,
    COMMENT,
    BLOCK_COMMENT,
    DOC_COMMENT,

    // Tokens outside the PHP code
    INLINE_HTML,
    OPEN_TAG,
    OPEN_TAG_ECHO,
    CLOSE_TAG,
    HALT_DATA,
    UNKNOWN,
    // Never part of a tree; what the parser sees past the last token.
    EOF,

    // Names, variables and literals
    IDENT,
    QUALIFIED_NAME,
    FULLY_QUALIFIED_NAME,
    RELATIVE_NAME,
    VARIABLE,
    INT_LITERAL,
    FLOAT_LITERAL,
    STRING_LITERAL,
    STRING_CONTENT,
    DOUBLE_QUOTE,
    BACKTICK,
    HEREDOC_START,
    HEREDOC_END,
    MAGIC_CONSTANT,
    CAST,

    // Punctuation
    LPAREN,
    RPAREN,
    LBRACKET,
    RBRACKET,
    LBRACE,
    RBRACE,
    CURLY_OPEN,
    DOLLAR_OPEN_CURLY,
    SEMICOLON,
    COMMA,
    COLON,
    DOUBLE_COLON,
    ARROW,
    NULLSAFE_ARROW,
    FAT_ARROW,
    ELLIPSIS,
    DOLLAR,
    BACKSLASH,
    AT,
    HASH_BRACKET,
    QUESTION,

    // Operators
    PLUS,
    MINUS,
    STAR,
    SLASH,
    PERCENT,
    POW,
    DOT,
    ASSIGN,
    PLUS_ASSIGN,
    MINUS_ASSIGN,
    STAR_ASSIGN,
    SLASH_ASSIGN,
    DOT_ASSIGN,
    PERCENT_ASSIGN,
    POW_ASSIGN,
    COALESCE_ASSIGN,
    AMP_ASSIGN,
    PIPE_ASSIGN,
    CARET_ASSIGN,
    SHL_ASSIGN,
    SHR_ASSIGN,
    EQ,
    NEQ,
    IDENTICAL,
    NOT_IDENTICAL,
    LT,
    GT,
    LE,
    GE,
    SPACESHIP,
    AND_AND,
    OR_OR,
    BANG,
    AMP,
    PIPE,
    CARET,
    TILDE,
    SHL,
    SHR,
    COALESCE,
    INC,
    DEC,
    PIPE_GT,

    // Keywords
    ABSTRACT_KW,
    AND_KW,
    ARRAY_KW,
    AS_KW,
    BREAK_KW,
    CALLABLE_KW,
    CASE_KW,
    CATCH_KW,
    CLASS_KW,
    CLONE_KW,
    CONST_KW,
    CONTINUE_KW,
    DECLARE_KW,
    DEFAULT_KW,
    DO_KW,
    ECHO_KW,
    ELSE_KW,
    ELSEIF_KW,
    EMPTY_KW,
    ENDDECLARE_KW,
    ENDFOR_KW,
    ENDFOREACH_KW,
    ENDIF_KW,
    ENDSWITCH_KW,
    ENDWHILE_KW,
    EVAL_KW,
    EXIT_KW,
    EXTENDS_KW,
    FINAL_KW,
    FINALLY_KW,
    FN_KW,
    FOR_KW,
    FOREACH_KW,
    FUNCTION_KW,
    GLOBAL_KW,
    GOTO_KW,
    HALT_COMPILER_KW,
    IF_KW,
    IMPLEMENTS_KW,
    INCLUDE_KW,
    INCLUDE_ONCE_KW,
    INSTANCEOF_KW,
    INSTEADOF_KW,
    INTERFACE_KW,
    ISSET_KW,
    LIST_KW,
    MATCH_KW,
    NAMESPACE_KW,
    NEW_KW,
    OR_KW,
    PRINT_KW,
    PRIVATE_KW,
    PROTECTED_KW,
    PUBLIC_KW,
    READONLY_KW,
    REQUIRE_KW,
    REQUIRE_ONCE_KW,
    RETURN_KW,
    STATIC_KW,
    SWITCH_KW,
    THROW_KW,
    TRAIT_KW,
    TRY_KW,
    UNSET_KW,
    USE_KW,
    VAR_KW,
    WHILE_KW,
    XOR_KW,
    YIELD_KW,

    // Nodes: the file and recovery
    SOURCE_FILE,
    ERROR,
    STATEMENT_LIST,
    NAME,

    // Nodes: statements
    EXPR_STATEMENT,
    ECHO_STATEMENT,
    BLOCK,
    EMPTY_STATEMENT,
    IF_STATEMENT,
    ELSEIF_CLAUSE,
    ELSE_CLAUSE,
    WHILE_STATEMENT,
    DO_WHILE_STATEMENT,
    FOR_STATEMENT,
    FOREACH_STATEMENT,
    SWITCH_STATEMENT,
    CASE_CLAUSE,
    DEFAULT_CLAUSE,
    BREAK_STATEMENT,
    CONTINUE_STATEMENT,
    RETURN_STATEMENT,
    GLOBAL_STATEMENT,
    STATIC_VARIABLE_STATEMENT,
    STATIC_VARIABLE,
    UNSET_STATEMENT,
    TRY_STATEMENT,
    CATCH_CLAUSE,
    FINALLY_CLAUSE,
    GOTO_STATEMENT,
    LABEL_STATEMENT,
    DECLARE_STATEMENT,
    DECLARE_DIRECTIVE,
    CONST_STATEMENT,
    CONST_ELEMENT,
    NAMESPACE_DECLARATION,
    USE_STATEMENT,
    USE_CLAUSE,
    USE_GROUP,
    HALT_COMPILER_STATEMENT,

    // Nodes: declarations
    FUNCTION_DECLARATION,
    CLASS_DECLARATION,
    INTERFACE_DECLARATION,
    TRAIT_DECLARATION,
    ENUM_DECLARATION,
    CLASS_BODY,
    EXTENDS_CLAUSE,
    IMPLEMENTS_CLAUSE,
    ENUM_BACKING_TYPE,
    METHOD_DECLARATION,
    PROPERTY_DECLARATION,
    PROPERTY_ELEMENT,
    CLASS_CONST_DECLARATION,
    TRAIT_USE,
    TRAIT_ADAPTATIONS,
    TRAIT_PRECEDENCE,
    TRAIT_ALIAS,
    ENUM_CASE,
    PROPERTY_HOOK_LIST,
    PROPERTY_HOOK,
    MODIFIER_LIST,
    PARAMETER_LIST,
    PARAMETER,
    RETURN_TYPE,
    ATTRIBUTE_LIST,
    ATTRIBUTE,
    ANONYMOUS_CLASS,

    // Nodes: types
    NAMED_TYPE,
    NULLABLE_TYPE,
    UNION_TYPE,
    INTERSECTION_TYPE,
    PAREN_TYPE,

    // Nodes: expressions
    LITERAL,
    VARIABLE_EXPR,
    VARIABLE_VARIABLE,
    ARRAY_EXPR,
    ARRAY_ITEM,
    LIST_EXPR,
    PREFIX_EXPR,
    POSTFIX_EXPR,
    BINARY_EXPR,
    ASSIGN_EXPR,
    TERNARY_EXPR,
    CAST_EXPR,
    CLONE_EXPR,
    NEW_EXPR,
    PRINT_EXPR,
    EXIT_EXPR,
    ISSET_EXPR,
    EMPTY_EXPR,
    EVAL_EXPR,
    INCLUDE_EXPR,
    THROW_EXPR,
    YIELD_EXPR,
    YIELD_FROM_EXPR,
    MATCH_EXPR,
    MATCH_ARM,
    CLOSURE_EXPR,
    CLOSURE_USE,
    CLOSURE_USE_VARIABLE,
    ARROW_FUNCTION_EXPR,
    PAREN_EXPR,
    CALL_EXPR,
    ARGUMENT_LIST,
    ARGUMENT,
    PROPERTY_FETCH_EXPR,
    SCOPED_ACCESS_EXPR,
    STATIC_PROPERTY_EXPR,
    INDEX_EXPR,
    SHELL_EXEC_EXPR,
    INTERPOLATED_STRING,
    HEREDOC,
    BRACED_INTERPOLATION,
    DOLLAR_BRACE_INTERPOLATION,
}

impl SyntaxKind {
    /// Whitespace and comments, which the parser keeps in the tree but never looks at.
    pub fn is_trivia(self) -> bool {
        matches!(
            self,
            SyntaxKind::WHITESPACE | SyntaxKind::COMMENT | SyntaxKind::BLOCK_COMMENT | SyntaxKind::DOC_COMMENT
        )
    }

    /// A reserved word, which PHP still accepts as a member, constant or label name in most places.
    pub fn is_keyword(self) -> bool {
        self >= SyntaxKind::ABSTRACT_KW && self <= SyntaxKind::YIELD_KW
    }

    /// A token as opposed to a node.
    pub fn is_token(self) -> bool {
        self < SyntaxKind::SOURCE_FILE
    }
}

/// The rowan language marker for PHP.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PhpLanguage {}

impl rowan::Language for PhpLanguage {
    type Kind = SyntaxKind;

    fn kind_from_raw(raw: rowan::SyntaxKind) -> SyntaxKind {
        SyntaxKind::ALL[raw.0 as usize]
    }

    fn kind_to_raw(kind: SyntaxKind) -> rowan::SyntaxKind {
        rowan::SyntaxKind(kind as u16)
    }
}

pub type SyntaxNode = rowan::SyntaxNode<PhpLanguage>;
pub type SyntaxToken = rowan::SyntaxToken<PhpLanguage>;
pub type SyntaxElement = rowan::SyntaxElement<PhpLanguage>;
