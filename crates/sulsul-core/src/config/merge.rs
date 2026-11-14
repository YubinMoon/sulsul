//! 기존 TOML 문서에 새 값을 덮어쓰면서 주석·순서·서식을 최대한 보존합니다.

use toml_edit::{DocumentMut, Item, Table, Value};

/// `src`의 값을 `dst`에 반영합니다. `dst`에만 있는 키(사용자가 추가한 항목)는 그대로 둡니다.
pub fn merge_document(dst: &mut DocumentMut, src: &DocumentMut) {
    merge_table(dst.as_table_mut(), src.as_table());
}

fn merge_table(dst: &mut Table, src: &Table) {
    for (key, src_item) in src.iter() {
        match dst.get_mut(key) {
            Some(dst_item) => merge_item(dst_item, src_item),
            None => {
                dst.insert(key, src_item.clone());
            }
        }
    }
}

fn merge_item(dst: &mut Item, src: &Item) {
    match (dst, src) {
        (Item::Table(dst), Item::Table(src)) => merge_table(dst, src),
        (Item::Value(dst), Item::Value(src)) => merge_value(dst, src),
        (dst, src) => *dst = src.clone(),
    }
}

fn merge_value(dst: &mut Value, src: &Value) {
    if let (Value::InlineTable(dst), Value::InlineTable(src)) = (&mut *dst, src) {
        for (key, src_value) in src.iter() {
            match dst.get_mut(key) {
                Some(dst_value) => merge_value(dst_value, src_value),
                None => {
                    dst.insert(key, src_value.clone());
                }
            }
        }
        return;
    }
    if bare(dst) == bare(src) {
        return;
    }
    let decor = dst.decor().clone();
    *dst = src.clone();
    *dst.decor_mut() = decor;
}

/// 앞뒤 공백·주석을 뺀 값 표현
fn bare(value: &Value) -> String {
    let mut value = value.clone();
    value.decor_mut().clear();
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_comments_and_unknown_keys() {
        let mut dst: DocumentMut =
            "# 머리 주석\n[a]\n# x 설명\nx = 1 # 꼬리 주석\nuser_key = true\n"
                .parse()
                .unwrap();
        let src: DocumentMut = "[a]\nx = 2\n[b]\ny = \"new\"\n".parse().unwrap();

        merge_document(&mut dst, &src);
        let text = dst.to_string();

        assert!(text.contains("# 머리 주석"));
        assert!(text.contains("# x 설명\nx = 2 # 꼬리 주석"));
        assert!(text.contains("user_key = true"));
        assert!(text.contains("[b]\ny = \"new\""));
    }

    #[test]
    fn unchanged_values_keep_formatting() {
        let mut dst: DocumentMut = "s = 'literal'\n".parse().unwrap();
        let src: DocumentMut = "s = 'literal'\n".parse().unwrap();
        merge_document(&mut dst, &src);
        assert_eq!(dst.to_string(), "s = 'literal'\n");
    }
}
