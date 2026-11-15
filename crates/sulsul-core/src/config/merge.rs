//! 기존 TOML 문서에 새 값을 덮어쓰면서 주석·순서·서식을 최대한 보존합니다.

use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, Value};

/// 배열 테이블(`[[...]]`)의 항목끼리 짝을 맞출 때 쓰는 키
const ID_KEY: &str = "id";

/// `src`의 값을 `dst`에 반영합니다. `dst`에만 있는 키(사용자가 추가한 항목)는 그대로 둡니다.
pub fn merge_document(dst: &mut DocumentMut, src: &DocumentMut) {
    merge_table(dst.as_table_mut(), src.as_table());
}

fn merge_table(dst: &mut Table, src: &Table) {
    for (key, src_item) in src.iter() {
        match dst.get_mut(key) {
            Some(dst_item) => merge_item(dst_item, src_item),
            None => {
                dst.insert(key, fresh_item(src_item));
            }
        }
    }
}

fn merge_item(dst: &mut Item, src: &Item) {
    match (dst, src) {
        (Item::Table(dst), Item::Table(src)) => merge_table(dst, src),
        (Item::Value(dst), Item::Value(src)) => merge_value(dst, src),
        (Item::ArrayOfTables(dst), Item::ArrayOfTables(src)) => merge_array_of_tables(dst, src),
        (dst, src) => *dst = fresh_item(src),
    }
}

/// 배열 테이블은 `id`가 같은 항목끼리(없으면 같은 자리끼리) 합쳐서, 남아 있는 항목의 주석과
/// 사용자가 넣은 키를 지킵니다. 순서와 개수는 `src`를 따르고, `src`에 없는 항목은 지웁니다.
fn merge_array_of_tables(dst: &mut ArrayOfTables, src: &ArrayOfTables) {
    let mut old: Vec<Option<Table>> = dst.iter().cloned().map(Some).collect();
    let mut merged = ArrayOfTables::new();
    for (index, src_table) in src.iter().enumerate() {
        let slot = match table_id(src_table) {
            Some(id) => old
                .iter_mut()
                .find(|slot| slot.as_ref().and_then(table_id) == Some(id)),
            None => old.get_mut(index),
        };
        let table = match slot.and_then(Option::take) {
            Some(mut table) => {
                merge_table(&mut table, src_table);
                table
            }
            None => {
                let mut table = src_table.clone();
                clear_positions(&mut table);
                table
            }
        };
        merged.push(table);
    }
    *dst = merged;
}

fn table_id(table: &Table) -> Option<&str> {
    table.get(ID_KEY).and_then(Item::as_str)
}

/// `src` 문서에서 가져온 항목을 `dst`에 새로 넣을 수 있게 복사합니다.
///
/// 테이블의 위치 번호는 문서마다 따로 매겨지므로 그대로 옮기면 엉뚱한 곳에 끼어들 수 있습니다.
/// 위치를 비워 두면 바로 앞 테이블(보통 부모나 앞 형제) 뒤에 놓입니다.
fn fresh_item(src: &Item) -> Item {
    let mut item = src.clone();
    match &mut item {
        Item::Table(table) => clear_positions(table),
        Item::ArrayOfTables(array) => array.iter_mut().for_each(clear_positions),
        Item::None | Item::Value(_) => {}
    }
    item
}

fn clear_positions(table: &mut Table) {
    table.set_position(None);
    for (_, item) in table.iter_mut() {
        match item {
            Item::Table(child) => clear_positions(child),
            Item::ArrayOfTables(array) => array.iter_mut().for_each(clear_positions),
            Item::None | Item::Value(_) => {}
        }
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
    fn array_of_tables_merge_by_id_and_keep_comments() {
        let mut dst: DocumentMut =
            "[s]\nx = 1\n\n# 첫 프로필\n[[s.p]]\nid = \"a\"\nv = 1 # 꼬리\nmine = true\n\n\
             # 둘째 프로필\n[[s.p]]\nid = \"b\"\nv = 2\n\n[t]\ny = 1\n"
                .parse()
                .unwrap();
        // b를 지우고 a를 고치고 c를 새로 넣습니다. 위치 번호는 dst와 다르게 매겨집니다.
        let src: DocumentMut =
            "[s]\nx = 1\n[[s.p]]\nid = \"c\"\nv = 3\n[[s.p]]\nid = \"a\"\nv = 10\n[t]\ny = 1\n"
                .parse()
                .unwrap();

        merge_document(&mut dst, &src);
        let text = dst.to_string();

        assert!(
            text.contains("# 첫 프로필\n[[s.p]]\nid = \"a\"\nv = 10 # 꼬리\nmine = true"),
            "남은 항목의 주석·직접 넣은 키가 지켜져야 합니다:\n{text}"
        );
        assert!(
            !text.contains("id = \"b\""),
            "지운 항목이 남았습니다:\n{text}"
        );
        assert!(
            !text.contains("# 둘째 프로필"),
            "지운 항목의 주석이 남았습니다"
        );
        let reparsed: DocumentMut = text.parse().unwrap();
        let ids: Vec<_> = reparsed["s"]["p"]
            .as_array_of_tables()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(ids, ["c", "a"], "순서는 새 값을 따라야 합니다");
        let t_pos = text.find("[t]").unwrap();
        assert!(
            text.find("id = \"c\"").unwrap() < t_pos,
            "새 항목은 부모 테이블 안쪽(다음 테이블 앞)에 놓여야 합니다:\n{text}"
        );
    }

    #[test]
    fn new_nested_table_follows_its_parent() {
        let mut dst: DocumentMut = "[a]\nx = 1\n\n[b]\ny = 1\n".parse().unwrap();
        // src에서는 [a.sub]가 문서 끝 쪽 위치 번호를 갖습니다.
        let src: DocumentMut = "[b]\ny = 1\n[a]\nx = 1\n[a.sub]\nz = 2\n".parse().unwrap();
        merge_document(&mut dst, &src);
        let text = dst.to_string();
        let sub = text.find("[a.sub]").unwrap();
        assert!(
            text.find("[a]").unwrap() < sub && sub < text.find("[b]").unwrap(),
            "새 하위 테이블은 부모 바로 뒤에 와야 합니다:\n{text}"
        );
    }

    #[test]
    fn unchanged_values_keep_formatting() {
        let mut dst: DocumentMut = "s = 'literal'\n".parse().unwrap();
        let src: DocumentMut = "s = 'literal'\n".parse().unwrap();
        merge_document(&mut dst, &src);
        assert_eq!(dst.to_string(), "s = 'literal'\n");
    }
}
