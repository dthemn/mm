//! The front page for visitors without an account on this device, in English, Polish, Russian and Ukrainian.
//! The rest of the app is in English; only this page is translated.

use crate::app::{Ctx, Screen};
use crate::icons::{Icon, icon};
use leptos::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Pl,
    Ru,
    Uk,
}

impl Lang {
    const ALL: [Lang; 4] = [Lang::En, Lang::Pl, Lang::Ru, Lang::Uk];

    fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Pl => "pl",
            Lang::Ru => "ru",
            Lang::Uk => "uk",
        }
    }

    /// The language's own name, as its speakers write it.
    fn name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Pl => "Polski",
            Lang::Ru => "Русский",
            Lang::Uk => "Українська",
        }
    }

    fn from_code(s: &str) -> Option<Lang> {
        let s = s.to_ascii_lowercase();
        Lang::ALL.into_iter().find(|l| s.starts_with(l.code()))
    }
}

/// The language picked before on this device, else the browser's first language we have, else English.
fn initial() -> Lang {
    let Some(w) = web_sys::window() else { return Lang::En };
    let saved = w.local_storage().ok().flatten().and_then(|s| s.get_item("mmLang").ok().flatten());
    if let Some(l) = saved.as_deref().and_then(Lang::from_code) {
        return l;
    }
    let nav = w.navigator();
    nav.languages().iter().filter_map(|v| v.as_string()).chain(nav.language())
        .find_map(|c| Lang::from_code(&c)).unwrap_or(Lang::En)
}

fn remember(l: Lang) {
    if let Some(w) = web_sys::window() {
        if let Some(s) = w.local_storage().ok().flatten() {
            let _ = s.set_item("mmLang", l.code());
        }
        if let Some(root) = w.document().and_then(|d| d.document_element()) {
            let _ = root.set_attribute("lang", l.code());
        }
    }
}

struct Text {
    headline: &'static str,
    lead: &'static str,
    create: &'static str,
    sign_in: &'static str,
    dark: &'static str,
    light: &'static str,
    dark_alt: &'static str,
    light_alt: &'static str,
    get: &'static str,
    features: [(&'static str, &'static str); 6],
    sees: (&'static str, &'static str),
    how: &'static str,
    steps: [(&'static str, &'static str); 3],
    open_source: &'static str,
    pill_open: &'static str,
    pill_self: &'static str,
    host_title: &'static str,
    host_intro: &'static str,
    host_guide: &'static str,
}

const REPO: &str = "https://github.com/dthemn/mm";
const GUIDE: &str = "https://github.com/dthemn/mm/blob/main/docs/DEPLOY.md";

fn text(l: Lang) -> &'static Text {
    match l {
        Lang::En => &EN,
        Lang::Pl => &PL,
        Lang::Ru => &RU,
        Lang::Uk => &UK,
    }
}

const EN: Text = Text {
    headline: "Every expense, kept like a receipt.",
    lead: "A money manager for spending and income. Photograph a receipt and your phone reads it. Your data is encrypted before it leaves the device.",
    create: "Sign up",
    sign_in: "Sign in",
    dark: "Dark",
    light: "Light",
    dark_alt: "Home in the dark theme: spent and income for the month, then operations by day",
    light_alt: "Stats in the light theme: a ring of spending by category with shares",
    get: "What you get",
    features: [
        ("Photo to saved receipt", "3 taps"),
        ("Text recognition", "on phone"),
        ("Lines by category", "automatic"),
        ("Bank CSV import", "no doubles"),
        ("Charts, search", "any item"),
        ("Every user", "own key"),
    ],
    sees: ("Server sees", "ciphertext"),
    how: "How it works",
    steps: [
        ("Create an account", "With a passkey: Face ID, a fingerprint or the device code. Write down 12 backup words."),
        ("Add what you spend", "A receipt photo, a bank statement, or by hand. The app finds the shop, date, total and every line."),
        ("See where it goes", "By day, month and category, with a search for any item you bought."),
    ],
    open_source: "open source",
    pill_open: "Open source · MIT",
    pill_self: "Self-hosted",
    host_title: "Host it yourself",
    host_intro: "mm is open source under the MIT license. Run it on your own server with Docker: your data stays on a machine you control, and even that server stores only ciphertext.",
    host_guide: "Self-hosting guide on GitHub",
};

const PL: Text = Text {
    headline: "Każdy wydatek zapisany jak na paragonie.",
    lead: "Menedżer finansów: wydatki i przychody w jednym miejscu. Zrób zdjęcie paragonu, a telefon sam go odczyta. Dane są szyfrowane, zanim opuszczą urządzenie.",
    create: "Załóż konto",
    sign_in: "Zaloguj się",
    dark: "Ciemny",
    light: "Jasny",
    dark_alt: "Ekran główny w ciemnym motywie: wydatki i przychody w miesiącu, potem operacje według dni",
    light_alt: "Statystyki w jasnym motywie: wykres kołowy wydatków według kategorii",
    get: "Co dostajesz",
    features: [
        ("Zdjęcie → paragon", "3 dotknięcia"),
        ("Rozpoznawanie tekstu", "na telefonie"),
        ("Pozycje wg kategorii", "automatycznie"),
        ("Import wyciągu CSV", "bez duplikatów"),
        ("Wykresy, szukanie", "każdy produkt"),
        ("Każdy użytkownik", "własny klucz"),
    ],
    sees: ("Serwer widzi", "tylko szyfr"),
    how: "Jak to działa",
    steps: [
        ("Załóż konto", "Z kluczem dostępu (passkey): Face ID, odcisk palca lub kod urządzenia. Zapisz 12 słów zapasowych."),
        ("Dodawaj wydatki", "Zdjęcie paragonu, wyciąg z banku albo ręcznie. Aplikacja znajdzie sklep, datę, sumę i każdą pozycję."),
        ("Zobacz, na co idą pieniądze", "Według dni, miesięcy i kategorii, z wyszukiwaniem dowolnego produktu."),
    ],
    open_source: "otwarty kod",
    pill_open: "Otwarty kod · MIT",
    pill_self: "Na własnym serwerze",
    host_title: "Uruchom u siebie",
    host_intro: "mm to otwarty kod na licencji MIT. Uruchom go na własnym serwerze z Dockerem: dane zostają na maszynie, którą kontrolujesz, a nawet ten serwer przechowuje tylko szyfr.",
    host_guide: "Instrukcja instalacji na GitHubie",
};

const RU: Text = Text {
    headline: "Каждый расход — как на чеке.",
    lead: "Финансовый трекер расходов и доходов. Сфотографируйте чек — телефон сам его прочитает. Данные шифруются ещё до того, как покинут устройство.",
    create: "Регистрация",
    sign_in: "Войти",
    dark: "Тёмная",
    light: "Светлая",
    dark_alt: "Главный экран в тёмной теме: расходы и доходы за месяц, затем операции по дням",
    light_alt: "Статистика в светлой теме: круговая диаграмма расходов по категориям",
    get: "Что вы получаете",
    features: [
        ("Фото → сохранённый чек", "3 касания"),
        ("Распознавание текста", "на телефоне"),
        ("Строки по категориям", "автоматически"),
        ("Импорт выписки CSV", "без дублей"),
        ("Графики, поиск", "любой товар"),
        ("Каждый пользователь", "свой ключ"),
    ],
    sees: ("Сервер видит", "только шифр"),
    how: "Как это работает",
    steps: [
        ("Создайте аккаунт", "С passkey: Face ID, отпечаток пальца или код устройства. Запишите 12 резервных слов."),
        ("Добавляйте расходы", "Фото чека, банковская выписка или вручную. Приложение найдёт магазин, дату, сумму и каждую строку."),
        ("Смотрите, куда уходят деньги", "По дням, месяцам и категориям, с поиском любого купленного товара."),
    ],
    open_source: "открытый код",
    pill_open: "Открытый код · MIT",
    pill_self: "На своём сервере",
    host_title: "Установите у себя",
    host_intro: "mm — открытый код под лицензией MIT. Запустите его на своём сервере с Docker: данные остаются на машине, которой управляете вы, и даже этот сервер хранит только шифр.",
    host_guide: "Инструкция по установке на GitHub",
};

const UK: Text = Text {
    headline: "Кожна витрата — як на чеку.",
    lead: "Фінансовий трекер витрат і доходів. Сфотографуйте чек — телефон сам його прочитає. Дані шифруються ще до того, як залишать пристрій.",
    create: "Реєстрація",
    sign_in: "Увійти",
    dark: "Темна",
    light: "Світла",
    dark_alt: "Головний екран у темній темі: витрати й доходи за місяць, далі операції за днями",
    light_alt: "Статистика у світлій темі: кругова діаграма витрат за категоріями",
    get: "Що ви отримуєте",
    features: [
        ("Фото → збережений чек", "3 дотики"),
        ("Розпізнавання тексту", "на телефоні"),
        ("Рядки за категоріями", "автоматично"),
        ("Імпорт виписки CSV", "без дублів"),
        ("Графіки, пошук", "будь-який товар"),
        ("Кожен користувач", "власний ключ"),
    ],
    sees: ("Сервер бачить", "лише шифр"),
    how: "Як це працює",
    steps: [
        ("Створіть акаунт", "З passkey: Face ID, відбиток пальця або код пристрою. Запишіть 12 резервних слів."),
        ("Додавайте витрати", "Фото чека, банківська виписка або вручну. Застосунок знайде магазин, дату, суму й кожен рядок."),
        ("Дивіться, куди йдуть гроші", "За днями, місяцями й категоріями, з пошуком будь-якого купленого товару."),
    ],
    open_source: "відкритий код",
    pill_open: "Відкритий код · MIT",
    pill_self: "На власному сервері",
    host_title: "Встановіть у себе",
    host_intro: "mm — відкритий код під ліцензією MIT. Запустіть його на власному сервері з Docker: дані залишаються на машині, якою керуєте ви, і навіть цей сервер зберігає лише шифр.",
    host_guide: "Інструкція зі встановлення на GitHub",
};

/// "Money manager (mm)": the app's name wherever it is introduced.
pub fn brand() -> impl IntoView {
    view! { <p class="brand" data-test="brand">"Money manager ("<span class="logo">"mm"</span>")"</p> }
}

pub fn welcome(c: Ctx) -> impl IntoView {
    let lang = RwSignal::new(initial());
    remember(lang.get_untracked());
    let register = move |_| c.screen.set(Screen::NewAccount);
    let login = move |_| c.screen.set(Screen::Locked);
    let switch = Lang::ALL.into_iter().map(|l| view! {
        <button data-test=format!("lang-{}", l.code()) lang=l.code() class:active=move || lang.get() == l aria-pressed=move || (lang.get() == l).to_string()
            on:click=move |_| { lang.set(l); remember(l); }>{l.name()}</button>
    }).collect_view();
    let page = move || {
        let t = text(lang.get());
        let lines = t.features.iter().map(|(k, v)| view! { <li class="line"><span>{*k}</span><span class="num">{*v}</span></li> }).collect_view();
        let steps = t.steps.iter().map(|(k, v)| view! { <li><strong>{*k}</strong><span class="hint">{*v}</span></li> }).collect_view();
        view! {
            <h1 data-test="headline">{t.headline}</h1>
            <div class="pills" data-test="pills">
                <a class="pill" href=REPO rel="noopener">{icon(Icon::Code)}{t.pill_open}</a>
                <a class="pill" href="#self-host">{icon(Icon::Server)}{t.pill_self}</a>
            </div>
            <p class="lead">{t.lead}</p>
            <div class="cta">
                <button class="primary" data-test="go-register" on:click=register>{t.create}</button>
                <button data-test="go-login" on:click=login>{t.sign_in}</button>
            </div>
            <div class="shots" data-test="shots">
                <figure>
                    <img src="/shot-dark.webp" width="390" height="844" alt=t.dark_alt />
                    <figcaption>{t.dark}</figcaption>
                </figure>
                <figure>
                    <img src="/shot-light.webp" width="390" height="844" alt=t.light_alt />
                    <figcaption>{t.light}</figcaption>
                </figure>
            </div>
            <div class="slip receipt">
                <p class="receipt-head">{t.get}</p>
                <ul class="receipt-lines">{lines}</ul>
                <p class="line total"><span>{t.sees.0}</span><strong>{t.sees.1}</strong></p>
            </div>
            <h2>{t.how}</h2>
            <ol class="steps">{steps}</ol>
            <section class="host" id="self-host" data-test="self-host">
                <h2>{icon(Icon::Server)}{t.host_title}</h2>
                <p class="hint">{t.host_intro}</p>
                <a class="guide" href=GUIDE rel="noopener" data-test="host-guide">{t.host_guide}" →"</a>
            </section>
            <div class="cta">
                <button class="primary" on:click=register>{t.create}</button>
                <button on:click=login>{t.sign_in}</button>
            </div>
            <footer class="foot" data-test="footer">
                <a href="mailto:contact@dthem.com">"contact@dthem.com"</a>
                <span class="hint">"Money manager (mm) · "{t.open_source}" · "<a href=REPO rel="noopener">"GitHub"</a></span>
            </footer>
        }
    };
    view! {
        <section class="landing" data-test="landing">
            {brand()}
            {page}
            <nav class="langs" aria-label="Language">{switch}</nav>
        </section>
    }
}
