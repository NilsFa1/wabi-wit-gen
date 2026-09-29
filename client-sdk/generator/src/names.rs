//! Case conversion for WIT kebab-case identifiers.

fn words(s: &str) -> impl Iterator<Item = &str> {
    s.split('-').filter(|w| !w.is_empty())
}

fn cap(w: &str) -> String {
    let mut c = w.chars();
    match c.next() {
        Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
        None => String::new(),
    }
}

/// `get-customer` -> `GetCustomer`
pub fn pascal(s: &str) -> String {
    words(s).map(cap).collect()
}

/// `get-customer` -> `getCustomer`
pub fn camel(s: &str) -> String {
    let p = pascal(s);
    let mut c = p.chars();
    match c.next() {
        Some(f) => f.to_ascii_lowercase().to_string() + c.as_str(),
        None => p,
    }
}

/// `get-customer` -> `get_customer`
pub fn snake(s: &str) -> String {
    words(s).collect::<Vec<_>>().join("_")
}

/// `get-customer` -> `GET_CUSTOMER`
pub fn screaming(s: &str) -> String {
    snake(s).to_ascii_uppercase()
}

/// Name of the exception/error wrapper generated for a WIT type used as the
/// error arm of a `result`: `client-error` -> `ClientException`.
pub fn exception_name(type_name: &str) -> String {
    let base = type_name.strip_suffix("-error").unwrap_or(type_name);
    format!("{}Exception", pascal(base))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions() {
        assert_eq!(pascal("get-customer"), "GetCustomer");
        assert_eq!(camel("get-customer"), "getCustomer");
        assert_eq!(snake("get-customer"), "get_customer");
        assert_eq!(screaming("get-customer"), "GET_CUSTOMER");
        assert_eq!(exception_name("client-error"), "ClientException");
        assert_eq!(exception_name("oops"), "OopsException");
    }
}
