// 协议适配自 Courseweave（MIT），凭据和 HTTP 访问改由 Courseflow 宿主提供。
use super::*;
impl FeatureClient<'_> {
    pub fn feature_exam_sso(&self) -> Result<()> {
        let url = format!(
            "https://passport.zhihuishu.com/cas/login?service={}",
            component(&format!("{HOMEWORK}/student/getStudentHomework"))
        );
        crate::zhs::fetch(self.host, FetchRequest::get(self.session, url))?;
        Ok(())
    }
    pub fn feature_papers(&self, course: &providers::Course) -> Result<Vec<Paper>> {
        if course.kind != "shared" {
            return self.smart_papers(course);
        }
        self.feature_exam_sso()?;
        let now = self.milliseconds()? as i64 / 1000;
        let mut result = vec![];
        let mut seen = HashSet::new();
        for (exam, flags) in [(false, vec![1, 2]), (true, vec![1, 0])] {
            for flag in flags {
                let base = if exam { FINAL } else { HOMEWORK };
                let endpoint = if exam {
                    "getStudentFinalExam"
                } else {
                    "getStudentHomework"
                };
                let reply=self.feature_request(&format!("{base}/student/{endpoint}"),&json!({"recruitId":remote_id(&course.recruit_id),"courseId":remote_id(&course.id),"flag":flag,"pageSize":200,"pageNum":0}),3,"POST",false,PlatformOperation::Read)?;
                let data = reply
                    .get("rt")
                    .or_else(|| reply.get("data"))
                    .ok_or_else(protocol_error)?;
                let empty = vec![];
                let rows = if exam {
                    data.as_array().ok_or_else(protocol_error)?
                } else {
                    match data.get("studentHomeworkList") {
                        None | Some(Value::Null) => &empty,
                        Some(Value::Array(rows)) => rows,
                        _ => return Err(protocol_error()),
                    }
                };
                for row in rows {
                    let exam_id = first(row, &["examId"]);
                    let student = first(row, &["id", "stuExamId"]);
                    if exam_id.is_empty()
                        || student.is_empty()
                        || !seen.insert((student.clone(), exam_id.clone()))
                    {
                        continue;
                    }
                    if row
                        .get("courseId")
                        .is_some_and(|v| !v.is_null() && net::text(v) != course.id)
                    {
                        continue;
                    }
                    let done = number(&row["state"]) == Some(4);
                    let starts = timestamp(&row["startTime"]);
                    let ends = timestamp(&row["endDate"]).or_else(|| timestamp(&row["endTime"]));
                    let left = if exam {
                        number(&row["faStudentExamRemainCount"])
                            .or_else(|| number(&row["answerCount"]))
                    } else {
                        number(&row["backNum"])
                            .filter(|n| *n >= 0)
                            .zip(number(&row["isMarking"]).or(Some(0)).filter(|n| *n >= 0))
                            .map(|(total, used)| (total - used).max(0))
                    };
                    let full = net::decimal(&row["score"])
                        .zip(net::decimal(&row["totalScore"]))
                        .is_some_and(|(score, total)| score >= total);
                    let redo = done && !exam && !full && left.is_some_and(|n| n > 0);
                    let mut reason = if starts.is_some_and(|n| n > now) {
                        "尚未开始"
                    } else if ends.is_some_and(|n| n <= now) {
                        "已截止"
                    } else if done && !redo {
                        if full {
                            "已满分，无需重做"
                        } else {
                            "平台已提交，当前没有可确认的重做次数"
                        }
                    } else if exam
                        && row["studentStartTime"].is_null()
                        && number(&row["state"]) != Some(2)
                    {
                        "请先在官方页面开考"
                    } else if exam && left.is_none_or(|n| n <= 0) {
                        "考试剩余次数不足或未知"
                    } else if !exam && !matches!(number(&row["state"]), Some(1 | 4 | 5)) {
                        "平台作答状态无法识别"
                    } else {
                        ""
                    };
                    if exam && reason.is_empty() {
                        let lock=self.feature_request(&format!("{HOMEWORK}/answer/getSaveAnswerLockResult"),&json!({"examId":remote_id(&exam_id),"recruitId":remote_id(&course.recruit_id)}),3,"POST",false,PlatformOperation::Read)?;
                        if number(&lock["rt"]["state"]) == Some(1) {
                            reason = "考试答案已锁定";
                        }
                    }
                    let mut native = row.clone();
                    native["_exam"] = json!(exam);
                    native["_exam_id"] = json!(exam_id);
                    native["_student"] = json!(student);
                    result.push(Paper {
                        can_redo: redo && reason.is_empty(),
                        id: format!("{}-{student}-{exam_id}", if exam { "sex" } else { "shw" }),
                        name: first(row, &["examName", "name"]),
                        category: if exam { "考试" } else { "作业" }.into(),
                        available: reason.is_empty(),
                        done,
                        reason: reason.into(),
                        status_label: if done { "已提交" } else { "未提交" }.into(),
                        score: net::decimal(&row["score"]),
                        attempts_left: left,
                        opens_at: starts,
                        ends_at: ends,
                        native,
                    });
                }
            }
        }
        Ok(result)
    }
    fn smart_papers(&self, course: &providers::Course) -> Result<Vec<Paper>> {
        let basic = self.feature_request(
            &format!("{SMART}/knowledge-study/course-basic"),
            &json!({"courseId":remote_id(&course.id),"classId":remote_id(&course.recruit_id)}),
            6,
            "POST",
            true,
            PlatformOperation::Read,
        )?;
        let mut papers = vec![];
        for point in knowledge_points(net::data(&basic)?) {
            let knowledge = first(&point, &["knowledgeId", "id"]);
            let reply=self.feature_request(&format!("{SMART}/exam/questions-paper"),&json!({"scMapId":remote_id(&course.id),"courseId":remote_id(&course.recruit_id),"classId":remote_id(&course.recruit_id),"knowledgeId":remote_id(&knowledge)}),6,"POST",true,PlatformOperation::Read)?;
            let info = net::data(&reply)?;
            let test = first(info, &["examTestId"]);
            let paper = first(info, &["paperId"]);
            if test.is_empty() || paper.is_empty() {
                continue;
            }
            let score = net::decimal(&info["highMasteryScore"]);
            let mut native = info.clone();
            native["_knowledge"] = json!(knowledge);
            native["_exam"] = json!(false);
            papers.push(Paper {
                can_redo: score.is_some_and(|n| n < 100.),
                id: format!("hw-{knowledge}-{test}-{paper}"),
                name: format!("{} · 作业", first(&point, &["knowledgeName", "name"])),
                category: "知识点作业".into(),
                available: score.is_none_or(|n| n < 100.),
                done: score.is_some_and(|n| n > 0.),
                reason: if score.is_some_and(|n| n >= 100.) {
                    "已满分"
                } else {
                    ""
                }
                .into(),
                status_label: if score.is_some_and(|n| n > 0.) {
                    "已取得掌握度"
                } else {
                    "未开始"
                }
                .into(),
                score,
                attempts_left: None,
                opens_at: None,
                ends_at: None,
                native,
            });
        }
        let reply = self.feature_request(
            "https://kg-run-student.zhihuishu.com/student/gateway/t/task/taskList",
            &json!({"courseId":course.id,"taskType":1,"taskName":"","status":0}),
            6,
            "POST",
            true,
            PlatformOperation::Read,
        )?;
        for row in net::data(&reply)?.as_array().ok_or_else(protocol_error)? {
            let task = first(row, &["id"]);
            let test = first(row, &["examTestId"]);
            let paper = first(row, &["examPaperId"]);
            if task.is_empty() || test.is_empty() || paper.is_empty() {
                continue;
            }
            let now = self.milliseconds()? as i64 / 1000;
            let starts = timestamp(&row["startTime"]);
            let ends = timestamp(&row["endTime"]);
            let reason = if starts.is_some_and(|t| t > now) {
                "尚未开始"
            } else if ends.is_some_and(|t| t <= now) {
                "已截止"
            } else {
                ""
            };
            let mut native = row.clone();
            native["_exam"] = json!(true);
            native["paperId"] = json!(paper);
            papers.push(Paper {
                can_redo: false,
                id: format!("exam-{task}-{test}-{paper}"),
                name: first(row, &["taskName", "name"]),
                category: "考试".into(),
                available: reason.is_empty(),
                done: false,
                reason: reason.into(),
                status_label: "待核对".into(),
                score: net::decimal(&row["score"]),
                attempts_left: None,
                opens_at: starts,
                ends_at: ends,
                native,
            });
        }
        Ok(papers)
    }
    pub fn feature_open(&self, course: &providers::Course, paper: &Paper) -> Result<Attempt> {
        if course.kind != "shared" {
            return self.smart_open(course, paper);
        }
        let school = self.feature_request(
            &format!("{HOMEWORK}/exam/getLoginSchoolInfo"),
            &json!({}),
            3,
            "POST",
            false,
            PlatformOperation::Read,
        )?;
        let school = first(&school["rt"], &["schoolId"]);
        let exam = paper.native["_exam"] == true;
        let base = if exam { FINAL } else { HOMEWORK };
        let exam_id = first(&paper.native, &["_exam_id", "examId"]);
        let student = first(&paper.native, &["_student", "id"]);
        if paper.done && exam {
            return Err(ModuleError::rejected(
                "共享课考试暂未提供可确认的只读查看路径，请导出本机已保存的执行报告",
            ));
        }
        let endpoint = if paper.done && !exam {
            "lookHomework"
        } else if exam {
            "doExam"
        } else {
            "doHomework"
        };
        let reply=self.feature_request(&format!("{base}/student/{endpoint}"),&json!({"recruitId":remote_id(&course.recruit_id),"examId":remote_id(&exam_id),"studentExamId":remote_id(&student),"schoolId":remote_id(&school),"courseId":remote_id(&course.id),"deviceId":""}),3,"POST",false,if paper.done{PlatformOperation::Read}else{PlatformOperation::OpenStudy})?;
        let data = reply
            .get("rt")
            .or_else(|| reply.get("data"))
            .ok_or_else(protocol_error)?;
        let parts = data["examBase"]["workExamParts"]
            .as_array()
            .ok_or_else(protocol_error)?;
        let mut questions = vec![];
        let mut mapping = serde_json::Map::new();
        for part in parts {
            for raw in part["questionDtos"].as_array().ok_or_else(protocol_error)? {
                let mut q = parse_question(raw, true)?;
                if paper.done {
                    providers::attach_review(&mut q, raw, false);
                }
                mapping.insert(q.id.clone(), raw.clone());
                questions.push(q);
            }
        }
        if questions.is_empty()
            || questions
                .iter()
                .map(|q| &q.id)
                .collect::<HashSet<_>>()
                .len()
                != questions.len()
            || number(&data["examBase"]["questionCount"])
                .is_some_and(|n| n != questions.len() as i64)
        {
            return Err(protocol_error());
        }
        let ids = questions
            .iter()
            .map(|q| q.id.clone())
            .collect::<Vec<_>>()
            .join(",");
        let answers=self.feature_request(&format!("{HOMEWORK}/answer/getStuAnswerInfo"),&json!({"recruitId":remote_id(&course.recruit_id),"stuExamId":remote_id(&student),"examId":remote_id(&exam_id),"schoolId":remote_id(&school),"courseId":remote_id(&course.id),"questionIds":ids}),3,"POST",false,PlatformOperation::Read)?;
        merge_existing(&mut questions, &answers);
        for q in &mut questions {
            if q.kind == "blank" && q.blanks > 1 && q.answer.len() == 1 {
                q.answer = q.answer[0].split('/').map(str::to_owned).collect();
            }
        }
        Ok(Attempt {
            name: paper.name.clone(),
            id: student.clone(),
            paper_id: paper.id.clone(),
            questions,
            expires_at: paper.ends_at,
            native: json!({"exam":exam,"exam_id":exam_id,"student":student,"school":school,"mapping":mapping,"shared":true,"readonly":paper.done}),
        })
    }
    fn smart_open(&self, course: &providers::Course, paper: &Paper) -> Result<Attempt> {
        let test = first(&paper.native, &["examTestId"]);
        let paper_id = first(&paper.native, &["paperId", "examPaperId"]);
        if test.is_empty() || paper_id.is_empty() {
            return Err(protocol_error());
        }
        let cache_key = format!(
            "{}/{}/{}/{}",
            course.id, course.recruit_id, paper.id, self.request
        );
        let mut cache = self.host.state_get("exam_open", &cache_key, self.session)?;
        if !cache.is_object() {
            cache = json!({});
        }
        if !paper.done && cache["opened"] != true {
            if cache["started"] == true {
                return Err(ModuleError::rejected(
                    "开卷请求结果未知，请到官方页面核对，不会重复开卷",
                ));
            }
            cache["started"] = json!(true);
            self.host
                .state_set("exam_open", &cache_key, &cache, self.session)?;
            if paper.native["_exam"] == true {
                self.feature_request("https://kg-ai-run.zhihuishu.com/run/gateway/t/task/exam/start-pull-paper",&json!({"courseId":remote_id(&course.id),"classId":remote_id(&course.recruit_id),"examTestId":remote_id(&test),"pageNumber":1,"pageSize":10}),6,"POST",true,PlatformOperation::OpenStudy)?;
            }
            let open = if paper.native["_exam"] == true {
                json!({"examTestId":remote_id(&test),"courseId":remote_id(&course.id)})
            } else {
                json!({"examTestId":remote_id(&test),"examPaperId":remote_id(&paper_id),"courseId":remote_id(&course.id)})
            };
            self.feature_request(
                &format!("{EXAM}/exam/user/openExam"),
                &open,
                5,
                "POST",
                false,
                PlatformOperation::OpenStudy,
            )?;
            cache["opened"] = json!(true);
            self.host
                .state_set("exam_open", &cache_key, &cache, self.session)?;
        }
        let sheet = if cache["sheet"].is_object() {
            cache["sheet"].clone()
        } else {
            let endpoint = if paper.native["_exam"] == true {
                "getAnswerSheetInformation"
            } else {
                "getExamSheetInfo"
            };
            let reply = self.feature_request(
                &format!("{EXAM}/exam/user/{endpoint}"),
                &json!({"examTestId":remote_id(&test),"examPaperId":remote_id(&paper_id)}),
                5,
                "GET",
                false,
                PlatformOperation::Read,
            )?;
            let mut sheet = net::data(&reply)?.clone();
            if !sheet["partSheetVos"].is_array() && sheet["questionSheetVos"].is_array() {
                sheet["partSheetVos"] = json!([{ "questionSheetVos": sheet["questionSheetVos"] }]);
            }
            cache["sheet"] = sheet.clone();
            self.host
                .state_set("exam_open", &cache_key, &cache, self.session)?;
            sheet
        };
        let mut entries = vec![];
        for part in sheet["partSheetVos"]
            .as_array()
            .ok_or_else(protocol_error)?
        {
            entries.extend(
                part["questionSheetVos"]
                    .as_array()
                    .ok_or_else(protocol_error)?
                    .iter()
                    .cloned(),
            );
        }
        if number(&sheet["questionCount"]).is_some_and(|n| n != entries.len() as i64) {
            return Err(protocol_error());
        }
        let mut questions: Vec<Question> =
            serde_json::from_value(cache["questions"].clone()).unwrap_or_default();
        let mut mapping = cache["mapping"].as_object().cloned().unwrap_or_default();
        let expected = entries.len();
        let cursor = questions.len();
        for entry in entries.into_iter().skip(cursor).take(6) {
            let id = first(&entry, &["questionId"]);
            let detail=self.feature_request(&format!("{EXAM}/question/getExamQuestionInfo"),&json!({"examTestId":remote_id(&test),"examPaperId":remote_id(&paper_id),"questionId":remote_id(&id),"version":entry.get("version").cloned().unwrap_or(json!(1))}),5,"GET",false,PlatformOperation::Read)?;
            let raw = net::data(&detail)?;
            let raw = raw.get("questionInfo").unwrap_or(raw);
            let mut q = parse_question(raw, false)?;
            q.id = id.clone();
            if paper.done {
                providers::attach_review(&mut q, raw, false);
            }
            if q.kind == "blank" && q.blanks > 1 {
                if let Some(answer) = raw
                    .get("userAnswer")
                    .or_else(|| raw.get("studentAnswer"))
                    .and_then(Value::as_str)
                {
                    let separator = if paper.native["_exam"] == true {
                        "/"
                    } else {
                        "#@#"
                    };
                    q.answer = answer.split(separator).map(str::to_owned).collect();
                }
            }
            let mut metadata = entry.clone();
            metadata["questionType"] = raw["questionType"].clone();
            mapping.insert(id, metadata);
            questions.push(q);
            cache["questions"] = json!(questions);
            cache["mapping"] = json!(mapping);
            self.host
                .state_set("exam_open", &cache_key, &cache, self.session)?;
        }
        if questions.is_empty() || mapping.len() != questions.len() {
            return Err(protocol_error());
        }
        let partial = questions.len() < expected;
        let cursor = questions.len();
        Ok(Attempt {
            name: paper.name.clone(),
            id: format!("{test}-{paper_id}"),
            paper_id: paper.id.clone(),
            questions,
            expires_at: paper.ends_at,
            native: json!({"partial":partial,"cursor":cursor,"shared":false,"test":test,"paper":paper_id,"knowledge":paper.native["_knowledge"],"exam":paper.native["_exam"],"mapping":mapping,"task":paper.native,"readonly":paper.done}),
        })
    }
    pub fn feature_writes(
        &self,
        course: &providers::Course,
        attempt: &Attempt,
        answers: &HashMap<String, Vec<String>>,
        submit: bool,
    ) -> Result<Vec<WriteRequest>> {
        let shared = attempt.native["shared"] == true;
        let exam = attempt.native["exam"] == true;
        let mut requests = vec![];
        if attempt.native["readonly"] == true {
            return Err(providers::unavailable("已提交的试卷只读，不会覆盖已有答案"));
        }
        for q in &attempt.questions {
            let answer = answers.get(&q.id).ok_or_else(protocol_error)?;
            if !providers::validate_answer(q, answer) {
                return Err(providers::unavailable("试卷中仍有不完整答案，请先人工补答"));
            }
            let answer = if q.kind == "blank" {
                answer.join(if shared {
                    "/"
                } else if exam {
                    "/"
                } else {
                    "#@#"
                })
            } else {
                answer.join(if shared { "," } else { "#@#" })
            };
            let (url, encoding, payload) = if shared {
                let raw = &attempt.native["mapping"][&q.id];
                let item = json!({"examId":attempt.native["exam_id"],"recruitId":remote_id(&course.recruit_id),"stuExamId":attempt.native["student"],"eid":remote_id(&q.id),"schoolId":attempt.native["school"],"deviceId":"","examType":if exam{json!(1)}else{json!("")},"fromType":3,"answer":answer,"dataIds":"","questionType":raw["questionType"]});
                let mut payload = json!({"stuExamAnswer":json!([item]).to_string(),"recruitId":remote_id(&course.recruit_id)});
                if exam {
                    payload["__source"] = json!(1);
                }
                (
                    format!("{HOMEWORK}/answer/saveStudentAnswer"),
                    "zhs_shared_exam",
                    payload,
                )
            } else {
                (
                    format!("{EXAM}/answer/saveAnswer"),
                    "zhs_exam",
                    json!({"recruitId":remote_id(&course.id),"examTestId":attempt.native["test"],"examPaperId":attempt.native["paper"],"questionId":remote_id(&q.id),"dataVos":null,"answer":answer}),
                )
            };
            requests.push(WriteRequest {
                url,
                method: "POST".into(),
                encoding: encoding.into(),
                payload,
                stage: "draft".into(),
            });
        }
        if !shared && exam {
            let answers = requests
                .iter()
                .map(|r| {
                    let mut answer = r.payload.clone();
                    let id = first(&answer, &["questionId"]);
                    answer["questionType"] = attempt.native["mapping"][&id]
                        .get("questionType")
                        .cloned()
                        .unwrap_or(Value::Null);
                    answer
                })
                .collect::<Vec<_>>();
            requests.clear();
            for batch in answers.chunks(20) {
                requests.push(WriteRequest{url:format!("{EXAM}/answer/saveBatchAnswer"),method:"POST".into(),encoding:"zhs_exam".into(),stage:"draft".into(),payload:json!({"examTestId":attempt.native["test"],"examPaperId":attempt.native["paper"],"recruitId":remote_id(&course.id),"answerList":batch})});
            }
        }
        if submit {
            let (url, encoding, payload) = if shared {
                (
                    format!("{}/answer/submit", if exam { FINAL } else { HOMEWORK }),
                    "zhs_shared_exam",
                    json!({"recruitId":remote_id(&course.recruit_id),"examId":attempt.native["exam_id"],"stuExamId":attempt.native["student"],"achieveCount":attempt.questions.len().to_string()}),
                )
            } else {
                (
                    format!("{EXAM}/exam/user/submit"),
                    "zhs_exam",
                    if exam {
                        json!({"examTestId":attempt.native["test"],"examPaperId":attempt.native["paper"],"recruitId":remote_id(&course.id)})
                    } else {
                        json!({"examTestId":attempt.native["test"],"examPaperId":attempt.native["paper"],"courseId":remote_id(&course.id),"courseType":8,"aiKnlowledgeId":attempt.native["knowledge"]})
                    },
                )
            };
            requests.push(WriteRequest {
                url,
                method: "POST".into(),
                encoding: encoding.into(),
                payload,
                stage: "submit".into(),
            });
        }
        Ok(requests)
    }
    pub fn feature_submitted(&self, course: &providers::Course, attempt: &Attempt) -> Result<bool> {
        if attempt.native["shared"] == true {
            return Ok(self
                .feature_papers(course)?
                .iter()
                .any(|p| p.id == attempt.paper_id && p.done));
        }
        let reply = self.feature_request(
            &format!("{EXAM}/exam/user/getExamSheetInfo"),
            &json!({"examTestId":attempt.native["test"],"examPaperId":attempt.native["paper"]}),
            5,
            "GET",
            false,
            PlatformOperation::Read,
        )?;
        let data = net::data(&reply)?;
        Ok(data["isSubmit"] == true
            || number(&data["submitStatus"]) == Some(1)
            || number(&data["examUserInfo"]["state"]) == Some(4))
    }
}

impl FeatureClient<'_> {
    pub fn redo_write(course: &providers::Course, paper: &Paper) -> Option<WriteRequest> {
        (course.kind=="shared"&&paper.native["_exam"]!=true&&!paper.native["_exam_id"].is_null()).then(||WriteRequest{url:format!("{HOMEWORK}/student/saveCourseTran"),method:"POST".into(),encoding:"zhs_shared_exam".into(),stage:"reset".into(),payload:json!({"recruitId":remote_id(&course.recruit_id),"examId":paper.native["_exam_id"],"description":"","courseId":remote_id(&course.id)})})
    }
}
