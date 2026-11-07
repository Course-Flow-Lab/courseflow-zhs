use super::*;
pub(super) fn knowledge_points(value: &Value) -> Vec<Value> {
    if let Some(themes) = value["cakeThemeList"].as_array() {
        return themes
            .iter()
            .flat_map(|t| t["knowledgeList"].as_array().into_iter().flatten())
            .cloned()
            .collect();
    }
    value["knowledgeList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}
pub fn parse_question(raw: &Value, shared: bool) -> Result<Question> {
    let id = first(
        raw,
        if shared {
            &["eid", "id"]
        } else {
            &["id", "questionId"]
        },
    );
    if id.is_empty() {
        return Err(protocol_error());
    }
    let body = first(
        raw,
        if shared {
            &["name", "content"]
        } else {
            &["content", "name", "title"]
        },
    );
    let kind = match number(&raw["questionType"]) {
        Some(1 | 14) => "single",
        Some(2) => "multiple",
        Some(3) => "blank",
        _ => "unsupported",
    };
    let mut pictures = vec![];
    let text = providers::rich_text(&body, &mut pictures);
    let list = raw
        .get(if shared {
            "questionOptions"
        } else {
            "optionVos"
        })
        .or_else(|| raw.get("options"))
        .and_then(Value::as_array);
    let options = list
        .into_iter()
        .flatten()
        .map(|o| OptionItem {
            id: first(o, &["id", "optionId"]),
            text: providers::rich_text(&first(o, &["content", "name", "text"]), &mut pictures),
        })
        .collect::<Vec<_>>();
    let blanks = if kind == "blank" {
        options
            .len()
            .max(raw["blankCount"].as_u64().unwrap_or(0) as usize)
            .max(1)
    } else {
        0
    };
    let kind = if ["single", "multiple"].contains(&kind)
        && (options.len() < 2 || options.iter().any(|o| o.id.is_empty()))
    {
        "unsupported"
    } else {
        kind
    };
    let answer = providers::answer_strings(
        kind,
        raw.get("userAnswer")
            .or_else(|| raw.get("studentAnswer"))
            .unwrap_or(&Value::Null),
    );
    Ok(Question {
        correct_answer: vec![],
        explanation: String::new(),
        id,
        text,
        kind: kind.into(),
        options,
        answer,
        images: pictures,
        blanks,
        native: Value::Null,
    })
}
pub(super) fn merge_existing(questions: &mut [Question], reply: &Value) {
    let mut stack = vec![reply];
    while let Some(value) = stack.pop() {
        if let Some(rows) = value.as_array() {
            stack.extend(rows);
        }
        if let Some(object) = value.as_object() {
            let id = first(value, &["eid", "questionId", "id"]);
            if let Some(q) = questions.iter_mut().find(|q| q.id == id)
                && let Some(answer) = value.get("answer").or_else(|| value.get("studentAnswer"))
            {
                q.answer = providers::answer_strings(&q.kind, answer);
            }
            for child in object.values() {
                if child.is_array() || child.is_object() {
                    stack.push(child);
                }
            }
        }
    }
}

pub(super) fn remote_id(id: &str) -> Value {
    id.parse::<u64>()
        .map(Value::from)
        .unwrap_or_else(|_| json!(id))
}
