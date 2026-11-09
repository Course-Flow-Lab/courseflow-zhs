//! 成绩分区单独读取与公共指标转换；缺失数值保留为空，不估算平台得分。
use super::operations::request;
use super::*;
use crate::zhs::catalog;

fn value(data: &Value, names: &[&str]) -> Value {
    names
        .iter()
        .find_map(|name| {
            let item = &data[*name];
            item.as_f64()
                .or_else(|| {
                    item.as_str()?
                        .trim()
                        .trim_end_matches('%')
                        .parse::<f64>()
                        .ok()
                })
                .filter(|n| n.is_finite() && *n >= 0.)
        })
        .map_or(Value::Null, |n| json!(n))
}
fn metric(label: &str, value: Value, max: Option<f64>, unit: &str) -> Value {
    json!({"label":label,"value":value,"max":max,"unit":unit})
}
pub(super) fn section(part: &str, raw: Value) -> Result<Value> {
    let (title, description, metrics, note) = match part {
        "scores" => (
            "成绩概览",
            "总评和各项得分来自平台成绩单。",
            vec![
                metric(
                    "当前总评",
                    value(&raw["scores"], &["totalScore"]),
                    None,
                    "分",
                ),
                metric(
                    "平时成绩",
                    value(&raw["scores"], &["usualScore"]),
                    None,
                    "分",
                ),
                metric(
                    "视频学习",
                    value(&raw["scores"], &["courseScore", "videoScore"]),
                    None,
                    "分",
                ),
                metric(
                    "章节测验",
                    value(&raw["scores"], &["examScore", "workScore"]),
                    None,
                    "分",
                ),
                metric("见面课", value(&raw["scores"], &["meetScore"]), None, "分"),
                metric(
                    "期末考试",
                    value(&raw["scores"], &["finalScore"]),
                    None,
                    "分",
                ),
            ],
            "各项可能已按课程规则折算，请勿直接相加推算总评。",
        ),
        "habit" => (
            "学习习惯",
            "查看持续学习带来的习惯分。",
            vec![metric("已获习惯分", raw["habit"].clone(), None, "分")],
            "以平台的学习天数和计分规则为准，复习时长不会被当作额外完成记录。",
        ),
        "interaction" => (
            "问答互动",
            "有效数量由平台认定，统计可能延迟更新。",
            vec![
                metric("已获互动分", raw["interactive"].clone(), None, "分"),
                metric(
                    "有效提问",
                    value(
                        &raw["qa"],
                        &["validQuestionAmount", "effectiveQuestion", "questions"],
                    ),
                    Some(10.),
                    "条",
                ),
                metric(
                    "有效回答",
                    value(
                        &raw["qa"],
                        &["validAnswerAmount", "effectiveAnswer", "answers"],
                    ),
                    Some(20.),
                    "条",
                ),
                metric(
                    "收获回答",
                    value(&raw["qa"], &["getAnswer", "received"]),
                    None,
                    "条",
                ),
            ],
            "10 条提问／20 条回答为工作台默认计分目标；超出不保证继续加分。",
        ),
        "rules" => {
            let fields = [
                ("视频学习", "progressScoreShare"),
                ("学习习惯", "learnedHabitScoreShare"),
                ("课程互动", "learnedInteractiveScoreShare"),
                ("知识点", "learnedKnowledgeScoreShare"),
                ("章节测验", "examScoreShare"),
                ("见面课", "meetCourseScoreShare"),
                ("期末考试", "finalExamScoreShare"),
            ];
            let metrics = fields
                .iter()
                .map(|(label, field)| {
                    metric(label, value(&raw["rules"], &[*field]), Some(100.), "%")
                })
                .collect();
            (
                "考核占比",
                "各学习项目在本课程考核中的比例。",
                metrics,
                "课程规则可能更新，最终成绩以平台公布结果为准。",
            )
        }
        _ => return Err(ModuleError::invalid_params("未知成绩分区")),
    };
    let mut result = raw;
    result["section"] = json!(part);
    result["title"] = json!(title);
    result["description"] = json!(description);
    result["metrics"] = json!(metrics);
    result["note"] = json!(note);
    Ok(result)
}
pub(super) fn load(
    client: &FeatureClient,
    course: &catalog::Course,
    part: Option<&str>,
) -> Result<Value> {
    // 成绩站点拥有独立会话，直接访问 JSON 接口可能得到全空的成功响应。
    super::operations::open_student_portal(client, course)?;
    let payload =
        json!({"courseId":course.id,"recruitId":course.recruit_id,"type":"l","courseTypeEs":1});
    let mut raw = json!({});
    if part.is_none() || part == Some("rules") {
        raw["rules"] =
            request(client, "loadStuLearingTab/", payload.clone())?["scoreAssessRuleDto"].clone();
    }
    if part.is_none() || part == Some("scores") {
        let mut query = payload.clone();
        query["stuRankType"] = json!("classRank");
        raw["scores"] = request(client, "loadStuLearnReport/", query)?["ssd"].clone();
    }
    if part.is_none() || part == Some("habit") {
        raw["habit"] =
            request(client, "loadStuHabit/", payload.clone())?["learnedHabitScore"].clone();
    }
    if part.is_none() || part == Some("interaction") {
        let data = request(client, "loadCourseForum/", payload)?;
        raw["interactive"] = data["learnedInteractiveScore"].clone();
        raw["qa"] = data["qaStatisticsMap"]["myInteractionDto"].clone();
        if value(
            &raw["qa"],
            &["validQuestionAmount", "effectiveQuestion", "questions"],
        )
        .is_null()
            && value(
                &raw["qa"],
                &["validAnswerAmount", "effectiveAnswer", "answers"],
            )
            .is_null()
        {
            return Err(ModuleError::unavailable(
                "平台未返回问答计数，请刷新或重新连接智慧树后重试",
            ));
        }
    }
    match part {
        Some(id) => section(id, raw),
        None => Ok(raw),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn score_sections_keep_missing_values_and_zero_distinct() {
        let result = section(
            "scores",
            json!({"scores":{"totalScore":0,"courseScore":"30.5"}}),
        )
        .unwrap();
        assert_eq!(result["metrics"][0]["value"], 0.);
        assert!(result["metrics"][1]["value"].is_null());
        assert_eq!(result["metrics"][2]["value"], 30.5);
        let qa = section(
            "interaction",
            json!({"interactive":3,"qa":{"validAnswerAmount":"23"}}),
        )
        .unwrap();
        assert_eq!(qa["metrics"][2]["value"], 23.);
        assert_eq!(qa["metrics"][2]["max"], 20.);
        assert!(section("unknown", json!({})).is_err());
    }
}
