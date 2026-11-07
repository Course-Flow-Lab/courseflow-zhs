// 协议实现遵循 MIT 许可，凭据和 HTTP 访问改由 Courseflow 宿主提供。
use super::*;
impl FeatureClient<'_> {
    pub fn mastery_papers(&self, course: &providers::Course) -> Result<Vec<Paper>> {
        if course.kind != "shared" {
            return self.feature_papers(course).map(|rows| {
                rows.into_iter()
                    .filter(|p| p.category == "知识点作业")
                    .map(|mut paper| {
                        paper.done = paper.score.is_some_and(|n| n >= 100.);
                        paper.can_redo = !paper.done;
                        paper
                    })
                    .collect()
            });
        }
        let map = self.feature_request(
            &format!("{MASTERY}/learning/queryCourseMapInfo"),
            &json!({"courseId":remote_id(&course.id),"recruitId":remote_id(&course.recruit_id)}),
            11,
            "GET",
            false,
            PlatformOperation::Read,
        )?;
        let map = net::data(&map)?;
        let hash = first(map, &["mapHashStr"]);
        let data = first(map, &["mapStr"]);
        if hash.is_empty() || data.is_empty() {
            return Err(protocol_error());
        }
        let req = FetchRequest::post_json(
            self.session,
            "https://share-course-map-service.zhihuishu.com/gateway/t/node/queryMasteryNodeInfo",
            json!({"idHash":hash,"idStr":data}),
        )
        .with_headers([(
            "Origin".into(),
            "https://studywisdomh5.zhihuishu.com".into(),
        )]);
        let response = crate::zhs::fetch_json(self.host, req)?;
        let tree = net::data(&response)?;
        let progress = self.feature_request(
            &format!("{MASTERY}/mastery/getAllMasteryInfos"),
            &json!({"recruitId":remote_id(&course.recruit_id)}),
            11,
            "GET",
            false,
            PlatformOperation::Read,
        )?;
        let mut rates = HashMap::new();
        for row in net::data(&progress)?
            .as_array()
            .ok_or_else(protocol_error)?
        {
            let rate = row["rate"]
                .as_f64()
                .or_else(|| row["rate"].as_str().and_then(|s| s.parse().ok()));
            rates.insert(first(row, &["knowledgeId"]), rate);
        }
        let mut pending = vec![];
        pending.extend(
            tree["nodeThemeRespVos"]
                .as_array()
                .into_iter()
                .flatten()
                .cloned(),
        );
        pending.extend(
            tree["randomNodes"]
                .as_array()
                .into_iter()
                .flatten()
                .cloned(),
        );
        let mut seen = HashSet::new();
        let mut result = vec![];
        while let Some(node) = pending.pop() {
            if result.len() + pending.len() > 10000 {
                return Err(protocol_error());
            }
            if let Some(children) = node["children"].as_array().filter(|v| !v.is_empty()) {
                pending.extend(children.iter().cloned());
                continue;
            }
            let id = first(&node, &["idHash"]);
            if id.is_empty() || !seen.insert(id.clone()) {
                continue;
            }
            let score = rates.get(&id).copied().flatten();
            let exempt = number(&node["freeExam"]) == Some(1);
            let done = exempt || score.is_some_and(|n| n >= 100.);
            result.push(Paper {
                can_redo: !done,
                id: format!("mastery-{id}"),
                name: first(&node, &["nodeName", "name"]),
                category: "掌握度".into(),
                available: !done,
                done,
                reason: if exempt {
                    "平台免考"
                } else if done {
                    "已达到 100%"
                } else {
                    ""
                }
                .into(),
                status_label: if exempt {
                    "免考"
                } else if done {
                    "已掌握"
                } else {
                    "待提升"
                }
                .into(),
                score,
                attempts_left: None,
                opens_at: None,
                ends_at: None,
                native: json!({"knowledge":id,"mastery":true}),
            });
        }
        Ok(result)
    }
    pub fn mastery_open(&self, course: &providers::Course, paper: &Paper) -> Result<Attempt> {
        if paper.native["mastery"] != true {
            return self.feature_open(course, paper);
        }
        let knowledge = first(&paper.native, &["knowledge"]);
        let reply=self.feature_request(&format!("{MASTERY}/exam/start"),&json!({"courseId":remote_id(&course.id),"recruitId":remote_id(&course.recruit_id),"knowledeId":knowledge}),11,"GET",false,PlatformOperation::OpenStudy)?;
        let data = net::data(&reply)?;
        let exam = first(data, &["examId"]);
        if exam.is_empty() {
            return Err(protocol_error());
        }
        let mut questions = vec![];
        for raw in data["questions"].as_array().ok_or_else(protocol_error)? {
            let mut normalized = raw.clone();
            normalized["id"] = raw["questionId"].clone();
            normalized["content"] = raw["questionName"].clone();
            if let Some(options) = normalized["optionVos"].as_array_mut() {
                for option in options {
                    option["content"] = option["name"].clone();
                }
            }
            questions.push(parse_question(&normalized, false)?);
        }
        if questions.is_empty() {
            return Err(protocol_error());
        }
        Ok(Attempt {
            name: paper.name.clone(),
            id: exam,
            paper_id: paper.id.clone(),
            questions,
            expires_at: None,
            native: json!({"mastery":true,"knowledge":knowledge}),
        })
    }
    pub fn mastery_writes(
        &self,
        course: &providers::Course,
        attempt: &Attempt,
        answers: &HashMap<String, Vec<String>>,
    ) -> Result<Vec<WriteRequest>> {
        if attempt.native["mastery"] != true {
            return self.feature_writes(course, attempt, answers, true);
        }
        let mut rows = vec![];
        for question in &attempt.questions {
            let answer = answers
                .get(&question.id)
                .filter(|a| providers::validate_answer(question, a))
                .ok_or_else(|| providers::unavailable("掌握度答案尚未完整"))?;
            let value = if question.kind == "judge" {
                if answer[0] == "true" { "1" } else { "0" }.to_owned()
            } else {
                answer.join(",")
            };
            rows.push(json!({"questionId":remote_id(&question.id),"answer":value}));
        }
        Ok(vec![WriteRequest {
            url: format!("{MASTERY}/exam/submit"),
            method: "POST".into(),
            encoding: "json".into(),
            stage: "submit".into(),
            payload: json!({"recruitId":remote_id(&course.recruit_id),"knowledgeId":attempt.native["knowledge"],"examId":remote_id(&attempt.id),"answerVos":rows,"dateFormate":self.milliseconds()?}),
        }])
    }
    pub fn mastery_submitted(&self, course: &providers::Course, attempt: &Attempt) -> Result<bool> {
        if attempt.native["mastery"] != true {
            return self.feature_submitted(course, attempt);
        }
        let result=self.feature_request(&format!("{MASTERY}/exam/getAnswerMasteryInfo"),&json!({"recruitId":remote_id(&course.recruit_id),"knowledeId":attempt.native["knowledge"],"examId":remote_id(&attempt.id)}),11,"GET",false,PlatformOperation::Read)?;
        let data = net::data(&result)?;
        Ok(data["rate"]
            .as_f64()
            .or_else(|| data["rate"].as_str().and_then(|s| s.parse().ok()))
            .is_some_and(|n| (0.0..=100.0).contains(&n)))
    }
    pub fn graded_answers(
        &self,
        course: &providers::Course,
        attempt: &Attempt,
    ) -> Result<HashMap<String, Vec<String>>> {
        if attempt.native["mastery"] != true {
            return Ok(HashMap::new());
        }
        let result=self.feature_request(&format!("{MASTERY}/exam/getUserAnswers"),&json!({"recruitId":remote_id(&course.recruit_id),"knowledeId":attempt.native["knowledge"],"examId":remote_id(&attempt.id)}),11,"GET",false,PlatformOperation::Read)?;
        let data = net::data(&result)?;
        let mut answers = HashMap::new();
        for raw in data["questions"].as_array().ok_or_else(protocol_error)? {
            if number(&raw["userAnswerVo"]["isCorrect"]) != Some(1) {
                continue;
            }
            let id = first(raw, &["questionId"]);
            if let Some(q) = attempt.questions.iter().find(|q| q.id == id) {
                if let Some(saved) = attempt.native["submitted_answers"]
                    .get(&id)
                    .and_then(Value::as_array)
                {
                    let answer = saved
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect::<Vec<_>>();
                    if providers::validate_answer(q, &answer) {
                        answers.insert(id, answer);
                        continue;
                    }
                }
                let value = raw["userAnswerVo"]
                    .get("answer")
                    .or_else(|| raw.get("answer"))
                    .unwrap_or(&Value::Null);
                let answer = providers::answer_strings(&q.kind, value);
                if providers::validate_answer(q, &answer) {
                    answers.insert(id, answer);
                }
            }
        }
        Ok(answers)
    }
}
