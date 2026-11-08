//! Contract examples for SDK copies of the shared ABI specification.
//! Only explicit documentation placeholders and known logical ABI function
//! names are expanded. Contract names and language identifiers are never guessed.
use crate::ir::{Model, Ret, Ty, TypeKind};

fn wit_type(t: &Ty) -> String {
    match t {
        Ty::Bool => "bool".into(), Ty::U8 => "u8".into(), Ty::U16 => "u16".into(),
        Ty::U32 => "u32".into(), Ty::U64 => "u64".into(), Ty::S8 => "s8".into(),
        Ty::S16 => "s16".into(), Ty::S32 => "s32".into(), Ty::S64 => "s64".into(),
        Ty::F32 => "f32".into(), Ty::F64 => "f64".into(), Ty::Char => "char".into(),
        Ty::String => "string".into(), Ty::Bytes => "list<u8>".into(),
        Ty::Named(name) => name.clone(),
        Ty::List(t) => format!("list<{}>", wit_type(t)),
        Ty::Option(t) => format!("option<{}>", wit_type(t)),
        Ty::Tuple(ts) => format!("tuple<{}>", ts.iter().map(wit_type).collect::<Vec<_>>().join(", ")),
        Ty::Stream(t) => format!("stream<{}>", wit_type(t)),
        Ty::Handle { resource, owned } => format!("{}<{resource}>", if *owned { "own" } else { "borrow" }),
    }
}

/// WIT excerpts contain only types and fields actually declared by this model.
fn contract_examples(m: &Model) -> String {
    let p = m.symbol_prefix();
    let mut s = format!("\n## Beispiele dieses Pakets\n\nDiese SDK-Ausgabe gehört zu `{}`. Alle folgenden Vertragsnamen stammen\naus dem eingelesenen WIT-Modell; es werden keine Referenzpaket-Typen ergänzt.\n\n- WIT-Paket: `{}:{}`\n- Symbolpräfix: `{p}_`\n- Native Bibliothek: `lib{p}.so`, `lib{p}.dylib` oder `{p}.dll`\n- Paket-Header: [`{p}.h`](../c/{p}.h)\n\n", m.contract_id(), m.namespace, m.package);
    if !m.resources.is_empty() {
        let mut names: Vec<_> = m.resources.iter().map(|r| r.name.as_str()).collect();
        names.sort_unstable();
        s.push_str(&format!("Ressourcen des Vertrags: {}.\n\n", names.iter().map(|n| format!("`{n}`")).collect::<Vec<_>>().join(", ")));
    }
    if let Some(t) = m.types.iter().filter(|t| matches!(t.kind, TypeKind::Record(_))).min_by_key(|t| &t.name) {
        if let TypeKind::Record(fields) = &t.kind {
            s.push_str("Record-Ausschnitt aus diesem Vertrag:\n\n```wit\n");
            s.push_str(&format!("record {} {{\n", t.name));
            for field in fields.iter().take(6) {
                s.push_str(&format!("    {}: {},\n", field.name, wit_type(&field.ty)));
            }
            if fields.len() > 6 { s.push_str("    // Weitere Felder stehen im WIT-Vertrag.\n"); }
            s.push_str("}\n```\n\n");
        }
    } else {
        s.push_str("Dieser Vertrag enthält keinen Record-Typ; ein fachliches Record-Beispiel entfällt.\n\n");
    }
    let exported = m.resources.iter().flat_map(|r| r.ctor.iter().chain(&r.methods).map(move |f| (r.name.as_str(), f)));
    let imported = m.imports.iter().flat_map(|i| i.funcs.iter().map(move |f| (i.name.as_str(), f)));
    let result = exported.filter(|(_, f)| matches!(f.ret, Ret::Result { .. }))
        .min_by_key(|(owner, f)| (*owner, f.name.as_str()))
        .or_else(|| imported.filter(|(_, f)| matches!(f.ret, Ret::Result { .. }))
            .min_by_key(|(owner, f)| (*owner, f.name.as_str())));
    if let Some((owner, f)) = result {
        if let Ret::Result { ok, err } = &f.ret {
            let arm = |t: &Option<Ty>| t.as_ref().map(wit_type).unwrap_or_else(|| "_".into());
            s.push_str(&format!("Ergebnistyp von `{owner}.{}`: `result<{}, {}>`.\nDie dort deklarierten Domänenfehler bleiben Payload-Daten und sind keine ABI-Statuscodes.\n\n", f.name, arm(ok), arm(err)));
        }
    } else {
        s.push_str("Dieser Vertrag verwendet keinen WIT-`result`-Rückgabetyp; ein fachliches Fehlerbeispiel entfällt.\n\n");
    }
    if m.imports.is_empty() {
        s.push_str("Dieser Vertrag enthält keine Host-Imports. Die Host-Call-Abschnitte der\ngemeinsamen Spezifikation beschreiben die optionalen ABI-Funktionen.\n\n");
    } else {
        let mut imports: Vec<_> = m.imports.iter().map(|i| i.name.as_str()).collect();
        imports.sort_unstable();
        s.push_str(&format!("Host-Imports dieses Vertrags: {}. Die App implementiert deren im WIT\ndeklarierte Funktionen.\n\n", imports.iter().map(|i| format!("`import {i};`")).collect::<Vec<_>>().join(", ")));
    }
    s.push_str("---\n");
    s
}

/// Project only exact logical function identifiers from the canonical ABI table.
/// This runs before inserting model values, which themselves may happen to have
/// the same spelling as an ABI operation.
fn function_names(m: &Model, text: &str) -> String {
    let names: std::collections::HashMap<_, _> = crate::abi_table::FUNCTIONS.iter()
        .map(|f| (f.name.to_string(), crate::abi_table::symbol(&m.symbol_prefix(), f.name))).collect();
    let mut out = String::with_capacity(text.len());
    let mut token = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || c == '_' { token.push(c); }
        else {
            out.push_str(names.get(&token).map(String::as_str).unwrap_or(&token));
            token.clear(); out.push(c);
        }
    }
    out.push_str(names.get(&token).map(String::as_str).unwrap_or(&token));
    out
}

pub fn render(m: &Model, name: &str, text: &str) -> String {
    let package = format!("{}:{}", m.namespace, m.package);
    let p = m.symbol_prefix();
    let expand = |text: &str| text.replace("<namespace>_<name>", &p).replace("<namespace>:<name>", &package)
        .replace("<symbol_prefix>", &p).replace("<prefix>", &p)
        .replace("<präfix>", &p).replace("<contract_id>", &m.contract_id());
    if name == "ABI.md" {
        let text = expand(&function_names(m, text));
        let (heading, rest) = text.split_once('\n').unwrap_or((&text, ""));
        return format!("{heading}\n{}{rest}", contract_examples(m));
    }
    let text = expand(text);
    // Header declarations remain the canonical WABI_FN macros. Only this
    // comment and explicit placeholders describe the concrete package.
    let example = if name == "wabi_abi_functions.h" {
        format!(" * Symbols: {p}_call_sync, {p}_future_take, ...\n")
    } else { String::new() };
    format!("/* ABI for {}.\n{example} */\n{text}", m.contract_id())
}
