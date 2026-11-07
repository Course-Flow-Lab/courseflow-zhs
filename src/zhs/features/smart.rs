use super::*;
impl FeatureClient<'_> {
    pub fn smart_units(&self, course: &providers::Course) -> Result<Vec<Unit>> {
        let basic = self.feature_request(
            &format!("{SMART}/knowledge-study/course-basic"),
            &json!({"courseId":remote_id(&course.id),"classId":remote_id(&course.recruit_id)}),
            6,
            "POST",
            true,
            PlatformOperation::Read,
        )?;
        let mut units = vec![];
        for point in knowledge_points(net::data(&basic)?) {
            let id = first(&point, &["knowledgeId", "id"]);
            let name = first(&point, &["knowledgeName", "name"]);
            let reply=self.feature_request(&format!("{SMART}/resources/list-knowledge-resource"),&json!({"courseId":remote_id(&course.id),"classId":remote_id(&course.recruit_id),"knowledgeId":remote_id(&id)}),6,"POST",true,PlatformOperation::Read)?;
            let resources = net::data(&reply)?["resourceList"]
                .as_array()
                .ok_or_else(protocol_error)?;
            for item in resources {
                let detail = item.get("resourcesDetail").unwrap_or(item);
                let uid = first(detail, &["resourcesUid", "resourcesFileId"]);
                let uid = if uid.is_empty() {
                    first(item, &["resourcesUid", "id"])
                } else {
                    uid
                };
                if uid.is_empty() {
                    return Err(protocol_error());
                }
                let file = first(detail, &["fileId", "resourcesFileId", "videoId"]);
                let kind = first(detail, &["resourcesType", "fileType", "type"]).to_lowercase();
                let distribution = number(&detail["resourcesDistributeType"])
                    .or_else(|| number(&item["resourcesDistributeType"]));
                let resource =
                    number(&detail["resourcesType"]).or_else(|| number(&item["resourcesType"]));
                let video = matches!(
                    (resource, distribution),
                    (Some(1), Some(3)) | (Some(2), Some(2))
                ) || ["video", "mp4", "flv", "m3u8"].contains(&kind.as_str());
                let mut duration = number(&detail["time"]).or_else(|| number(&detail["duration"]));
                if video && duration.is_none() {
                    let reply=self.feature_request(&format!("{SMART}/resources-lab/get-video-time"),&json!({"courseId":remote_id(&course.id),"classId":remote_id(&course.recruit_id),"videoIdList":[remote_id(&file)]}),6,"POST",true,PlatformOperation::Read)?;
                    duration = net::data(&reply)?
                        .as_array()
                        .and_then(|rows| rows.first())
                        .and_then(|r| number(&r["time"]));
                }
                let mut native = item.clone();
                native["_file"] = json!(file);
                native["_knowledge"] = json!(id);
                native["_resource"] = json!(uid);
                units.push(Unit {
                    id: uid,
                    name: first(detail, &["resourcesName", "name"]),
                    chapter_id: id.clone(),
                    chapter_name: name.clone(),
                    lesson_id: id.clone(),
                    kind: if video { "video" } else { "document" }.into(),
                    duration_seconds: duration,
                    watched_seconds: number(&item["lastWatchTime"]).or_else(|| {
                        duration
                            .zip(number(&item["schedule"]))
                            .map(|(d, p)| d * p.clamp(0, 100) / 100)
                    }),
                    state: if number(&item["studyStatus"]) == Some(1) {
                        "completed"
                    } else {
                        "not_started"
                    }
                    .into(),
                    video_id: Some(file),
                    native,
                });
            }
        }
        Ok(units)
    }
    pub fn smart_study_write(
        course: &providers::Course,
        unit: &Unit,
        position: i64,
        elapsed: i64,
        complete: bool,
    ) -> WriteRequest {
        let payload = if complete {
            json!({"courseId":remote_id(&course.id),"classId":remote_id(&course.recruit_id),"knowledgeId":unit.native["_knowledge"],"resourcesUid":unit.native["_resource"],"watchUId":1})
        } else {
            json!({"courseId":remote_id(&course.id),"classId":remote_id(&course.recruit_id),"fileId":unit.native["_file"],"knowledgeId":unit.native["_knowledge"],"lastWatchTime":position,"studyTotalTime":elapsed,"shareCourseId":"","nodeType":0,"watchUId":1})
        };
        WriteRequest {
            url: format!(
                "{SMART}/studyRecord/{}",
                if complete { "completed" } else { "report" }
            ),
            method: "POST".into(),
            encoding: "zhs_ai".into(),
            stage: "study".into(),
            payload,
        }
    }
}
