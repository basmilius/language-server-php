//! Types as PHP writes them in a signature.

use php_index::Type;

/// A native type as source, with every class written the way `class` says, or `None` for a type
/// that has no spelling in PHP (an array shape, a template, a conditional).
pub fn native_type_text(ty: &Type, class: &mut dyn FnMut(&str) -> String) -> Option<String> {
    Some(match ty {
        Type::Mixed => "mixed".to_string(),
        Type::Void => "void".to_string(),
        Type::Never => "never".to_string(),
        Type::Null => "null".to_string(),
        Type::Bool => "bool".to_string(),
        Type::True => "true".to_string(),
        Type::False => "false".to_string(),
        Type::Int | Type::IntLiteral(_) => "int".to_string(),
        Type::Float => "float".to_string(),
        Type::String | Type::StringLiteral(_) => "string".to_string(),
        Type::Object => "object".to_string(),
        Type::Array(..) | Type::List(_) | Type::Shape(_) => "array".to_string(),
        Type::Iterable(..) => "iterable".to_string(),
        Type::Callable(_) => "callable".to_string(),
        Type::Static => "static".to_string(),
        Type::SelfType => "self".to_string(),
        Type::Parent => "parent".to_string(),
        Type::Class { name, .. } => class(name),
        Type::Union(members) => {
            let nullable = members.len() == 2 && members.contains(&Type::Null);
            if nullable {
                let other = members.iter().find(|member| **member != Type::Null)?;
                if matches!(other, Type::Intersection(_) | Type::Union(_)) {
                    return None;
                }
                format!("?{}", native_type_text(other, class)?)
            } else {
                let parts: Option<Vec<String>> = members
                    .iter()
                    .map(|member| {
                        let text = native_type_text(member, class)?;
                        Some(if matches!(member, Type::Intersection(_)) {
                            format!("({text})")
                        } else {
                            text
                        })
                    })
                    .collect();
                parts?.join("|")
            }
        }
        Type::Intersection(members) => {
            let parts: Option<Vec<String>> = members.iter().map(|member| native_type_text(member, class)).collect();
            parts?.join("&")
        }
        _ => return None,
    })
}
