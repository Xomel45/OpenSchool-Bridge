//! Platform-neutral models exposed to Rust and Kotlin clients.

/// The student the logged-in account is bound to (`GET /api/myschool/v2/auth/student`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, uniffi::Record)]
pub struct Student {
    /// Pass this to `Client::week` / `Client::class_info`.
    pub id: String,
    pub first_name: String,
    pub last_name: String,
    pub middle_name: String,
    pub region: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, uniffi::Record)]
pub struct Teacher {
    pub first_name: String,
    pub last_name: String,
    pub patronymic: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, uniffi::Record)]
pub struct Absence {
    pub code: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, uniffi::Record)]
pub struct Lesson {
    pub id: String,
    pub subject_id: String,
    pub subject_name: String,
    pub teacher: Teacher,
    /// Local time, `YYYY-MM-DD HH:MM:SS`.
    pub start: String,
    pub end: String,
    pub room: String,
    /// Lesson topic; the server often leaves it empty.
    pub theme: Option<String>,
    pub absence: Option<Absence>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, uniffi::Record)]
pub struct Material {
    pub name: String,
    pub link: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, uniffi::Record)]
pub struct Homework {
    pub id: String,
    pub subject_id: Option<String>,
    pub subject_name: String,
    pub description: String,
    /// `YYYY-MM-DD`: the lesson it was assigned on.
    pub issue_date: String,
    /// `YYYY-MM-DD`: the lesson it is due on.
    pub due_date: String,
    /// Id of the lesson it is due on; equals `Lesson::id` of that lesson.
    pub due_lesson_id: Option<String>,
    pub materials: Vec<Material>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, uniffi::Record)]
pub struct Mark {
    pub id: String,
    pub subject_id: String,
    pub subject_name: String,
    /// Usually `"2"`..`"5"`; kept as a string because other scales and letters exist.
    pub value: String,
    /// Second part of a double mark, when present.
    pub value2: Option<String>,
    /// `YYYY-MM-DD`.
    pub date: String,
    /// E.g. `Ordinary`, `Test`.
    pub work_type_code: String,
    /// Human-readable work type, e.g. "Контрольная работа".
    pub work_type: String,
    pub work_name: Option<String>,
    pub comment: Option<String>,
    pub lesson_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, uniffi::Record)]
pub struct Period {
    pub number: u32,
    pub start_date: String,
    pub end_date: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, uniffi::Record)]
pub struct ClassInfo {
    pub school_name: String,
    pub class_number: String,
    pub class_letter: String,
    pub academic_year: u32,
    pub quarters: Vec<Period>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, uniffi::Record)]
pub struct Week {
    pub year: u32,
    pub iso_week: u32,
    pub lessons: Vec<Lesson>,
    pub homeworks: Vec<Homework>,
    pub marks: Vec<Mark>,
}
