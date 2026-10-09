//! Wire format of `POST /api/myschool/v1/datamart` and its mapping to public models.
//!
//! The request is a JSON array of `{interval_id, obj_type, student_id}` queries; the response is
//! an array of `{obj_type, interval_id, data: [...]}` in arbitrary order.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::error::BridgeError;
use crate::models::*;

#[derive(Serialize)]
pub(crate) struct Query<'a> {
    pub interval_id: String,
    pub obj_type: &'a str,
    pub student_id: &'a str,
}

/// Weekly intervals are `{iso_week}{year}`, e.g. week 41 of 2026 is `"412026"`.
pub(crate) fn week_interval(year: u32, iso_week: u32) -> String {
    format!("{iso_week}{year}")
}

pub(crate) fn week_queries<'a>(student_id: &'a str, year: u32, iso_week: u32) -> Vec<Query<'a>> {
    ["student_lessons", "student_homeworks_materials", "student_marks"]
        .into_iter()
        .map(|obj_type| Query { interval_id: week_interval(year, iso_week), obj_type, student_id })
        .collect()
}

pub(crate) fn class_queries(student_id: &str, year: u32) -> Vec<Query<'_>> {
    vec![Query { interval_id: year.to_string(), obj_type: "student_classes", student_id }]
}

/// The server sends `null` as often as it leaves a field out; for text and lists both mean "empty".
/// (`#[serde(default)]` alone only covers a missing field: one `null` would fail the whole week.)
fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

#[derive(Deserialize)]
struct Item {
    obj_type: String,
    #[serde(default, deserialize_with = "null_default")]
    interval_id: String,
    #[serde(default, deserialize_with = "null_default")]
    data: Vec<serde_json::Value>,
}

fn items(body: &str) -> Result<Vec<Item>, BridgeError> {
    serde_json::from_str(body).map_err(|e| BridgeError::Parse(e.to_string()))
}

fn rows<T: serde::de::DeserializeOwned>(items: &[Item], obj_type: &str) -> Result<Vec<T>, BridgeError> {
    items
        .iter()
        .filter(|i| i.obj_type == obj_type)
        .flat_map(|i| i.data.iter())
        .map(|v| T::deserialize(v).map_err(|e| BridgeError::Parse(e.to_string())))
        .collect()
}

/// The server returns `"2026-10-06 12:10:00.0"`; drop the fractional part.
fn clean_dt(s: &str) -> String {
    s.strip_suffix(".0").unwrap_or(s).to_string()
}

#[derive(Deserialize)]
struct RawLesson {
    lesson_id: String,
    subject_id: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    subject_name: String,
    #[serde(default, deserialize_with = "null_default")]
    firstname: String,
    #[serde(default, deserialize_with = "null_default")]
    lastname: String,
    #[serde(default, deserialize_with = "null_default")]
    patronymic: String,
    start_datetime: String,
    end_datetime: String,
    #[serde(default)]
    room: Option<String>,
    #[serde(default)]
    theme: Option<String>,
    skipping_id: Option<String>,
    type_code: Option<String>,
    type_description: Option<String>,
}

#[derive(Deserialize)]
struct RawMaterial {
    #[serde(default, deserialize_with = "null_default")]
    name: String,
    #[serde(default, deserialize_with = "null_default")]
    link: String,
}

#[derive(Deserialize)]
struct RawHomework {
    homeworks_id: String,
    subject_id: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    subject_name: String,
    #[serde(default, deserialize_with = "null_default")]
    description: String,
    issue_date: String,
    plan_ready_date: String,
    #[serde(default)]
    ready_lesson_id: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    materials: Vec<RawMaterial>,
}

#[derive(Deserialize)]
struct RawMark {
    marks_id: String,
    subject_id: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    subject_name: String,
    mark_value1: String,
    mark_value2: Option<String>,
    mark_date: String,
    #[serde(default, deserialize_with = "null_default")]
    work_type_code: String,
    #[serde(default, deserialize_with = "null_default")]
    work_type_description: String,
    work_name: Option<String>,
    comment: Option<String>,
    lesson_id: Option<String>,
}

#[derive(Deserialize)]
struct RawPeriod {
    period_id: String,
    period_num: String,
    period_start_date: String,
    period_end_date: String,
    period_type_code: String,
    #[serde(default)]
    period_is_study: bool,
}

#[derive(Deserialize)]
struct RawClass {
    #[serde(default, deserialize_with = "null_default")]
    short_name: String,
    #[serde(default, deserialize_with = "null_default")]
    class_num: String,
    #[serde(default, deserialize_with = "null_default")]
    class_letter: String,
    academ_year: u32,
    #[serde(default, deserialize_with = "null_default")]
    periods: Vec<RawPeriod>,
}

pub(crate) fn parse_week(body: &str, year: u32, iso_week: u32) -> Result<Week, BridgeError> {
    let mut items = items(body)?;
    // The reply may carry neighbouring weeks too, in any order, sometimes as thinner copies of the same lessons.
    // De-duplication keeps the first copy it meets, so put the week that was asked for first (the sort is stable).
    let own = week_interval(year, iso_week);
    items.sort_by_key(|i| i.interval_id != own);

    // A neighbouring week's interval may repeat lessons, so de-duplicate by id.
    let mut seen = HashSet::new();
    let mut lessons: Vec<Lesson> = rows::<RawLesson>(&items, "student_lessons")?
        .into_iter()
        .filter(|l| seen.insert(l.lesson_id.clone()))
        .map(|l| Lesson {
            id: l.lesson_id,
            subject_id: l.subject_id.unwrap_or_default(),
            subject_name: l.subject_name,
            teacher: Teacher { first_name: l.firstname, last_name: l.lastname, patronymic: l.patronymic },
            start: clean_dt(&l.start_datetime),
            end: clean_dt(&l.end_datetime),
            room: l.room.filter(|r| r != "-").unwrap_or_default(),
            theme: l.theme.filter(|t| !t.trim().is_empty()),
            absence: l.skipping_id.map(|_| Absence {
                code: l.type_code.unwrap_or_default(),
                description: l.type_description.unwrap_or_default(),
            }),
        })
        .collect();
    lessons.sort_by(|a, b| a.start.cmp(&b.start));

    let mut seen = HashSet::new();
    let mut homeworks: Vec<Homework> = rows::<RawHomework>(&items, "student_homeworks_materials")?
        .into_iter()
        .filter(|h| seen.insert(h.homeworks_id.clone()))
        .map(|h| Homework {
            id: h.homeworks_id,
            subject_id: h.subject_id,
            subject_name: h.subject_name,
            description: h.description,
            issue_date: h.issue_date,
            due_date: h.plan_ready_date,
            due_lesson_id: h.ready_lesson_id,
            materials: h.materials.into_iter().map(|m| Material { name: m.name, link: m.link }).collect(),
        })
        .collect();
    homeworks.sort_by(|a, b| a.due_date.cmp(&b.due_date));

    // Marks of a neighbouring week can be repeated too.
    let mut seen = HashSet::new();
    let mut marks: Vec<Mark> = rows::<RawMark>(&items, "student_marks")?
        .into_iter()
        .filter(|m| seen.insert(m.marks_id.clone()))
        .map(|m| Mark {
            id: m.marks_id,
            subject_id: m.subject_id.unwrap_or_default(),
            subject_name: m.subject_name,
            value: m.mark_value1,
            value2: m.mark_value2,
            date: m.mark_date,
            work_type_code: m.work_type_code,
            work_type: m.work_type_description,
            work_name: m.work_name,
            comment: m.comment,
            lesson_id: m.lesson_id,
        })
        .collect();
    marks.sort_by(|a, b| a.date.cmp(&b.date));

    Ok(Week { year, iso_week, lessons, homeworks, marks })
}

#[derive(Deserialize)]
struct RawStudent {
    student_id: String,
    #[serde(default, deserialize_with = "null_default")]
    student_first_name: String,
    #[serde(default, deserialize_with = "null_default")]
    student_last_name: String,
    #[serde(default, deserialize_with = "null_default")]
    student_middle_name: String,
    #[serde(default, deserialize_with = "null_default")]
    student_region: String,
}

/// `auth/student` returns an array (one entry per student linked to the account).
pub(crate) fn parse_students(body: &str) -> Result<Vec<Student>, BridgeError> {
    let raw: Vec<RawStudent> = serde_json::from_str(body).map_err(|e| BridgeError::Parse(e.to_string()))?;
    Ok(raw
        .into_iter()
        .map(|s| Student {
            id: s.student_id,
            first_name: s.student_first_name,
            last_name: s.student_last_name,
            middle_name: s.student_middle_name,
            region: s.student_region,
        })
        .collect())
}

pub(crate) fn parse_class(body: &str) -> Result<Option<ClassInfo>, BridgeError> {
    let items = items(body)?;
    let Some(c) = rows::<RawClass>(&items, "student_classes")?.into_iter().next() else {
        return Ok(None);
    };
    // The server lists every quarter twice (display and study variants); keep the study ones.
    let mut quarters: Vec<Period> = c
        .periods
        .into_iter()
        .filter(|p| p.period_type_code == "quarter" && p.period_is_study && !p.period_id.is_empty())
        .filter_map(|p| {
            Some(Period {
                number: p.period_num.parse().ok()?,
                start_date: p.period_start_date,
                end_date: p.period_end_date,
            })
        })
        .collect();
    quarters.sort_by_key(|p| p.number);
    Ok(Some(ClassInfo {
        school_name: c.short_name,
        class_number: c.class_num,
        class_letter: c.class_letter,
        academic_year: c.academ_year,
        quarters,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEEK: &str = r#"[
      {"obj_type":"student_lessons","interval_id":"412026","data":[
        {"lesson_id":"L2","subject_id":"S1","subject_name":"Math","firstname":"Ann","lastname":"Doe","patronymic":"X",
         "start_datetime":"2026-10-06 10:00:00.0","end_datetime":"2026-10-06 10:40:00.0","room":"-","theme":"Equations",
         "skipping_id":null,"type_code":null,"type_description":null},
        {"lesson_id":"L1","subject_id":"S2","subject_name":"Art","firstname":"Bob","lastname":"Roe","patronymic":"Y",
         "start_datetime":"2026-10-05 09:00:00.0","end_datetime":"2026-10-05 09:40:00.0","room":"12",
         "skipping_id":"K1","type_code":"notallowded","type_description":"No reason"}]},
      {"obj_type":"student_lessons","interval_id":"422026","data":[
        {"lesson_id":"L1","subject_id":"S2","subject_name":"Art","start_datetime":"2026-10-05 09:00:00.0","end_datetime":"2026-10-05 09:40:00.0"}]},
      {"obj_type":"student_homeworks_materials","interval_id":"412026","data":[
        {"homeworks_id":"H1","subject_id":null,"subject_name":"","description":"read p.5","issue_date":"2026-10-05","plan_ready_date":"2026-10-07","ready_lesson_id":"L2",
         "materials":[{"name":"a.docx","link":"https://example.test/a"}]}]}
    ]"#;

    #[test]
    fn the_asked_for_week_wins_over_a_thinner_copy_that_arrives_first() {
        // the neighbouring week (422026) comes first and lacks the teacher and the absence of L1
        let body = r#"[
          {"obj_type":"student_lessons","interval_id":"422026","data":[
            {"lesson_id":"L1","subject_id":"S2","subject_name":"Art","start_datetime":"2026-10-05 09:00:00.0","end_datetime":"2026-10-05 09:40:00.0"}]},
          {"obj_type":"student_lessons","interval_id":"412026","data":[
            {"lesson_id":"L1","subject_id":"S2","subject_name":"Art","firstname":"Bob","lastname":"Roe","patronymic":"Y",
             "start_datetime":"2026-10-05 09:00:00.0","end_datetime":"2026-10-05 09:40:00.0","skipping_id":"K1","type_code":"notallowded","type_description":"No reason"}]}
        ]"#;
        let w = parse_week(body, 2026, 41).unwrap();
        assert_eq!(w.lessons.len(), 1);
        assert_eq!(w.lessons[0].teacher.last_name, "Roe");
        assert!(w.lessons[0].absence.is_some(), "the absence must survive, whatever the order of the reply");
    }

    #[test]
    fn null_in_a_text_field_does_not_fail_the_whole_week() {
        let body = r#"[
          {"obj_type":"student_lessons","interval_id":"412026","data":[
            {"lesson_id":"L1","subject_id":null,"subject_name":null,"firstname":null,"lastname":null,"patronymic":null,
             "start_datetime":"2026-10-05 09:00:00.0","end_datetime":"2026-10-05 09:40:00.0"}]},
          {"obj_type":"student_homeworks_materials","interval_id":"412026","data":[
            {"homeworks_id":"H1","subject_name":null,"description":null,"issue_date":"2026-10-05","plan_ready_date":"2026-10-07","materials":null},
            {"homeworks_id":"H2","subject_name":"Art","description":"x","issue_date":"2026-10-05","plan_ready_date":"2026-10-07","materials":[{"name":null,"link":null}]}]},
          {"obj_type":"student_marks","interval_id":"412026","data":[
            {"marks_id":"M1","subject_name":null,"mark_value1":"5","mark_date":"2026-10-05","work_type_code":null,"work_type_description":null}]}
        ]"#;
        let w = parse_week(body, 2026, 41).unwrap();
        assert_eq!((w.lessons.len(), w.homeworks.len(), w.marks.len()), (1, 2, 1));
        assert_eq!(w.lessons[0].teacher.last_name, "");
        assert!(w.homeworks[0].materials.is_empty());
        let students = parse_students(r#"[{"student_id":"1","student_first_name":null,"student_last_name":"Doe","student_middle_name":null,"student_region":null}]"#).unwrap();
        assert_eq!((students[0].first_name.as_str(), students[0].last_name.as_str()), ("", "Doe"));
    }

    #[test]
    fn parses_and_dedupes_marks() {
        let body = r#"[
          {"obj_type":"student_marks","interval_id":"412026","data":[
            {"marks_id":"M2","subject_id":"S1","subject_name":"Math","mark_value1":"3","mark_value2":"2","mark_date":"2026-10-06",
             "work_type_code":"Test","work_type_description":"Test work","work_name":"quiz","comment":null,"lesson_id":"L1"},
            {"marks_id":"M1","subject_id":null,"subject_name":"Art","mark_value1":"5","mark_value2":null,"mark_date":"2026-10-05",
             "work_type_code":"Ordinary","work_type_description":"Ordinary","work_name":null,"comment":null,"lesson_id":null}]},
          {"obj_type":"student_marks","interval_id":"402026","data":[
            {"marks_id":"M1","subject_name":"Art","mark_value1":"5","mark_date":"2026-10-05"}]}
        ]"#;
        let w = parse_week(body, 2026, 41).unwrap();
        assert_eq!(w.marks.len(), 2);
        assert_eq!(w.marks[0].id, "M1");
        assert_eq!(w.marks[1].value2.as_deref(), Some("2"));
        assert_eq!(w.marks[1].work_name.as_deref(), Some("quiz"));
    }

    #[test]
    fn week_query_includes_marks() {
        let q = week_queries("s", 2026, 41);
        assert!(q.iter().any(|x| x.obj_type == "student_marks" && x.interval_id == "412026"));
    }

    /// Runs against a local, sanitized capture (scratch/ is git-ignored); skipped when absent.
    #[test]
    fn real_capture_parses_if_present() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/scratch/samples");
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.ends_with("-res.json") {
                continue;
            }
            let body = std::fs::read_to_string(e.path()).unwrap();
            let w = parse_week(&body, 2026, 41).unwrap_or_else(|err| panic!("{name}: {err}"));
            assert!(!w.lessons.is_empty() && !w.marks.is_empty(), "{name}");
            eprintln!("{name}: {} lessons, {} marks, {} hw", w.lessons.len(), w.marks.len(), w.homeworks.len());
        }
    }

    #[test]
    fn parses_students() {
        let body = r#"[{"trusted":true,"student_last_name":"Doe","student_first_name":"Ann","student_middle_name":"X",
          "student_id":"u-1","student_oid":5,"student_region":"41","student_birth_date":"2010-01-01"}]"#;
        let s = parse_students(body).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].id.as_str(), s[0].first_name.as_str(), s[0].region.as_str()), ("u-1", "Ann", "41"));
        assert!(parse_students("{}").is_err());
    }

    #[test]
    fn interval_format() {
        assert_eq!(week_interval(2026, 41), "412026");
    }

    #[test]
    fn parses_sorts_and_dedupes_week() {
        let w = parse_week(WEEK, 2026, 41).unwrap();
        assert_eq!(w.lessons.len(), 2);
        assert_eq!(w.lessons[0].id, "L1");
        assert_eq!(w.lessons[0].start, "2026-10-05 09:00:00");
        assert_eq!(w.lessons[0].absence.as_ref().unwrap().code, "notallowded");
        assert_eq!(w.lessons[1].room, "");
        assert_eq!(w.lessons[1].theme.as_deref(), Some("Equations"));
        assert_eq!(w.lessons[0].theme, None);
        assert_eq!(w.homeworks[0].materials[0].name, "a.docx");
        assert_eq!(w.homeworks[0].subject_id, None);
        assert_eq!(w.homeworks[0].due_lesson_id.as_deref(), Some("L2"));
    }

    #[test]
    fn parses_class_study_quarters_only() {
        let body = r#"[{"obj_type":"student_classes","data":[{"short_name":"School 1","class_num":"8","class_letter":"B","academ_year":2026,
          "periods":[
            {"period_id":"a","period_num":"2","period_start_date":"2026-11-03","period_end_date":"2026-12-29","period_type_code":"quarter","period_is_study":false},
            {"period_id":"b","period_num":"2","period_start_date":"2026-11-03","period_end_date":"2026-12-29","period_type_code":"quarter","period_is_study":true},
            {"period_id":"c","period_num":"1","period_start_date":"2026-09-01","period_end_date":"2026-10-27","period_type_code":"quarter","period_is_study":true}]}]}]"#;
        let c = parse_class(body).unwrap().unwrap();
        assert_eq!(c.quarters.iter().map(|p| p.number).collect::<Vec<_>>(), vec![1, 2]);
    }

    #[test]
    fn bad_json_is_parse_error() {
        assert!(matches!(parse_week("{", 2026, 1), Err(BridgeError::Parse(_))));
    }
}
