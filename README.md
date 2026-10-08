# OpenSchool Bridge

Общее ядро открытого клиента электронного дневника **«Моя школа»** (Госуслуги): запросы к API, разбор ответов и модели данных. На нём работает [OpenSchool-Desktop](https://github.com/Xomel45/OpenSchool-Desktop), позже на нём же будет Android-приложение на Kotlin (через [UniFFI](https://mozilla.github.io/uniffi-rs/)).

> Неофициальный проект, не связан с Госуслугами и «Моей школой». Библиотека делает те же запросы, что веб-интерфейс, под сессией самого пользователя, и только читает данные. Она не работает с паролями: вход через ЕСИА выполняет приложение, а сюда передаются готовые cookie сессии.

## Возможности

- Подключение сессии: список cookie или строка заголовка `Cookie` (как отдаёт `CookieManager` на Android).
- Проверка сессии: `check_session` (жива ли она, либо нужно войти заново).
- Ученик, привязанный к аккаунту: `students`.
- Неделя целиком одним вызовом `week`: уроки, домашние задания, оценки (повторяющиеся записи на стыке недель объединены).
- Класс и четверти учебного года: `class_info`.
- Ошибки трёх видов (`Network`, `Auth`, `Parse`), для HTTP-ошибок с началом тела ответа.
- Честный User-Agent: `OpenSchool/<версия> (+https://github.com/Xomel45/OpenSchool-Bridge)`.

Что не реализовано: школьные события (источник данных не найден), рейтинги, итоговые оценки за четверть и год (их нет в API до конца четверти), скачивание вложений.

## Пример (Rust)

```rust
use openschool_bridge::{Client, SessionCookie};

#[tokio::main]
async fn main() -> Result<(), openschool_bridge::BridgeError> {
    let client = Client::new()?;
    // Cookie сессии получает приложение после входа через ЕСИА (LOGIN_URL).
    client.set_session(vec![SessionCookie { name: "...".into(), value: "...".into() }]);

    if !client.check_session().await? {
        return Ok(()); // сессия устарела: запустить вход заново
    }
    let student = client.students().await?.remove(0);
    let week = client.week(student.id.clone(), 2026, 41).await?; // ISO-неделя 41 года 2026
    for lesson in &week.lessons {
        println!("{} {}", lesson.start, lesson.subject_name);
    }
    Ok(())
}
```

Вход через ЕСИА открывайте на `login_url()` (`https://www.gosuslugi.ru/school/feed`) во встроенном браузере и забирайте cookie после перехода обратно на `/school`.

## Сборка и тесты

```bash
cargo test
cargo build --release
```

На Windows для криптобиблиотеки (`aws-lc-rs`) нужен [NASM](https://www.nasm.us) в `PATH`.

### Kotlin / Android

```bash
# Kotlin-привязки из собранной библиотеки
cargo build
cargo run --features cli --bin uniffi-bindgen -- generate \
  --library target/debug/libopenschool_bridge.so --language kotlin --out-dir out

# библиотека для Android (нужны cargo-ndk и Android NDK)
cargo install cargo-ndk
rustup target add aarch64-linux-android
cargo ndk -t arm64-v8a build --release
```

Привязки генерируются и `arm64-v8a` собирается; запуск на настоящем Android-устройстве ещё не проверялся. Для HTTPS на Android может потребоваться инициализация проверки сертификатов через JNI при старте приложения.

## Структура

```
src/lib.rs        публичный API и константы
src/client.rs     Client: сессия, запросы, проверки
src/datamart.rs   запросы /api/myschool/v1/datamart и разбор ответов
src/models.rs     модели для Rust и Kotlin (UniFFI)
src/error.rs      BridgeError
```

## Лицензия

[MPL-2.0](LICENSE).
