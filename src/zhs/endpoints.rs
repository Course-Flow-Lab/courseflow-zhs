//! Real 智慧树 hosts and paths, as constants.
//!
//! Every request below is issued by the framework through `cw.net.fetch`; the module
//! itself never resolves a hostname. The reference implementation is
//! `courseweave/src/zhs.rs` (and `zhs/outline.rs`, `zhs/study.rs`, `zhs/features.rs`).

// --- Passport (QR login) ---------------------------------------------------------
pub const PASSPORT_BASE: &str = "https://passport.zhihuishu.com";
pub const PASSPORT_LOGIN_PATH: &str = "/login";
pub const QR_IMAGE_PATH: &str = "/qrCodeLogin/getLoginQrImg";
pub const QR_INFO_PATH: &str = "/qrCodeLogin/getLoginQrInfo";
pub const ABNORMAL_LOGIN_PATH: &str = "/user/abnormalLogin";

// --- Online service (student course catalogue / session verify) ------------------
pub const SERVICE_BASE: &str = "https://onlineservice-api.zhihuishu.com";
pub const SHARE_COURSES_PATH: &str = "/gateway/t/v1/student/course/share/queryShareCourseInfo";
pub const AI_COURSES_PATH: &str = "/gateway/t/v1/student/queryStudentAICourseList";
pub const LOGIN_USER_INFO_PATH: &str = "/gateway/f/v1/login/getLoginUserInfo";
pub const GO_LOGIN_PATH: &str = "/gateway/f/v1/login/gologin";
pub const COURSE_LOGIN_FROM: &str = "https://onlineweb.zhihuishu.com/onlinestuh5";

// --- Learning gateway (outline + study writes, protocol module 11) ----------------
pub const LEARNING_BASE: &str = "https://studyservice-api.zhihuishu.com";
pub const OUTLINE_VIDEOLIST_PATH: &str = "/gateway/t/v1/learning/videolist";
pub const OUTLINE_PROGRESS_PATH: &str = "/gateway/t/v1/learning/queryStuyInfo";
pub const STUDY_QUERY_COURSE_PATH: &str = "/gateway/t/v1/learning/queryCourse";
pub const STUDY_OPEN_PATH: &str = "/gateway/t/v1/learning/prelearningNote";
pub const STUDY_SAVE_PATH: &str = "/gateway/t/v1/learning/saveDatabaseIntervalTimeV2";
pub const STUDY_LIMIT_PATH: &str = "/gateway/t/v1/course/queryStudiedTimeLimitInfo";
pub const STUDY_POPUP_PATH: &str = "/gateway/t/v1/popupAnswer/loadVideoPointerInfo";
pub const POPUP_EXAM_PATH: &str = "/gateway/t/v1/popupAnswer/lessonPopupExam";
pub const POPUP_SAVE_PATH: &str = "/gateway/t/v1/popupAnswer/saveLessonPopupExamSaveAnswer";

// --- Smart / AI course gateway (protocol module 6) --------------------------------
pub const SMART_BASE: &str = "https://kg-ai-run.zhihuishu.com/run/gateway/t/stu";
pub const SMART_COURSE_BASIC_PATH: &str = "/knowledge-study/course-basic";
pub const SMART_RESOURCES_PATH: &str = "/resources/list-knowledge-resource";
pub const SMART_VIDEO_TIME_PATH: &str = "/resources-lab/get-video-time";

// --- Protocol key bootstrap (RSA + AES, not ported in this draft) -----------------
pub const KEY_SERVICE_URL: &str = "https://appcomm-user.zhihuishu.com/app-commserv-user/c/has";

// --- Papers / exams ---------------------------------------------------------------
pub const EXAM_BASE: &str = "https://newexam-api.zhihuishu.com/gateway/t/v1";
pub const HOMEWORK_BASE: &str = "https://studentexam-api.zhihuishu.com/studentExam/gateway/t/v1";
pub const FINAL_EXAM_BASE: &str = "https://taurusexam-api.zhihuishu.com/taurusExam/gateway/t/v1";

// --- Mastery ----------------------------------------------------------------------
pub const MASTERY_BASE: &str = "https://aistudy.zhihuishu.com/gateway/t/v1";

// --- Meetings ---------------------------------------------------------------------
pub const STU_ONLINE_BASE: &str = "https://stuonline.zhihuishu.com";
pub const MEETING_LIST_PATH: &str = "/stuonline/json/teachMeeting/listTeachMeetingStu";
pub const IM_BASE: &str = "https://im.zhihuishu.com";

// --- Q&A --------------------------------------------------------------------------
pub const QA_GATEWAY: &str = "https://creditqa-api.zhihuishu.com/creditqa/gateway/t/v1/web/qa/";
