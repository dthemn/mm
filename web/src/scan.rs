//! From a photo to a draft operation. The photo exists only in memory during recognition:
//! pixels go to the worker as tensors, and nothing is stored or sent to the server.

use crate::app::{Ctx, Screen, today};
use crate::finance::{Doubts, Draft};
use crate::icons::{Icon, icon};
use js_sys::{Array, Float32Array, Function, Object, Promise, Reflect};
use leptos::prelude::*;
use leptos::task::spawn_local;
use mm_core::categorize::{categorize, tidy_name};
use mm_core::crypto::merchant_id;
use mm_core::model::*;
use mm_core::ocr::{self, Line, Tensor};
use mm_core::receipt::{self, Doubt};
use mm_core::vision::{Gray, prepare};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::JsFuture;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, HtmlVideoElement, MediaStream, MessageEvent, Worker};

/// Long side of the image kept in memory. Larger photos add memory, not accuracy.
const MAX_SIDE: u32 = 2400;

fn e(x: JsValue) -> String {
    x.as_string().unwrap_or_else(|| format!("{x:?}"))
}

type Calls = Rc<RefCell<HashMap<u32, (Function, Function)>>>;

struct Ocr {
    worker: Worker,
    calls: Calls,
    next: Cell<u32>,
    dict: Vec<String>,
    _listener: Closure<dyn FnMut(MessageEvent)>,
}

thread_local! {
    static OCR: RefCell<Option<Rc<Ocr>>> = const { RefCell::new(None) };
}

impl Ocr {
    async fn get() -> Result<Rc<Ocr>, String> {
        if let Some(o) = OCR.with(|o| o.borrow().clone()) {
            return Ok(o);
        }
        let opts = web_sys::WorkerOptions::new();
        opts.set_type(web_sys::WorkerType::Module);
        let worker = Worker::new_with_options("/ocr-worker.js", &opts).map_err(e)?;
        let calls: Calls = Rc::default();
        let inbox = calls.clone();
        let listener = Closure::<dyn FnMut(MessageEvent)>::new(move |m: MessageEvent| {
            let d = m.data();
            let id = Reflect::get(&d, &"id".into()).ok().and_then(|v| v.as_f64()).unwrap_or(-1.0) as u32;
            let Some((ok, fail)) = inbox.borrow_mut().remove(&id) else { return };
            let err = Reflect::get(&d, &"error".into()).unwrap_or(JsValue::UNDEFINED);
            let _ = if err.is_undefined() { ok.call1(&JsValue::NULL, &d) } else { fail.call1(&JsValue::NULL, &err) };
        });
        worker.set_onmessage(Some(listener.as_ref().unchecked_ref()));
        let dict = fetch_text("/ocr/latin_dict.txt").await?;
        let dict: Vec<String> = dict.strip_suffix('\n').unwrap_or(&dict).split('\n').map(str::to_owned).collect();
        let o = Rc::new(Ocr { worker, calls, next: Cell::new(0), dict, _listener: listener });
        OCR.with(|c| *c.borrow_mut() = Some(o.clone()));
        Ok(o)
    }

    /// Runs `op` ("det" or "rec") and returns the output data and its shape.
    async fn run(&self, op: &str, t: Option<&Tensor>) -> Result<(Vec<f32>, Vec<usize>), String> {
        let id = self.next.get();
        self.next.set(id + 1);
        let calls = self.calls.clone();
        let done = Promise::new(&mut |ok, fail| {
            calls.borrow_mut().insert(id, (ok, fail));
        });
        let msg = Object::new();
        let set = |k: &str, v: &JsValue| Reflect::set(&msg, &k.into(), v).map(|_| ()).map_err(e);
        set("id", &id.into())?;
        set("op", &op.into())?;
        let transfer = Array::new();
        if let Some(t) = t {
            let input = Float32Array::from(t.data.as_slice());
            transfer.push(&input.buffer());
            set("input", &input)?;
            set("h", &(t.h as u32).into())?;
            set("w", &(t.w as u32).into())?;
        }
        self.worker.post_message_with_transfer(&msg, &transfer).map_err(e)?;
        let reply = JsFuture::from(done).await.map_err(|x| format!("recognizer failed: {}", e(x)))?;
        let data = Reflect::get(&reply, &"data".into()).map_err(e)?;
        if data.is_undefined() {
            return Ok((vec![], vec![]));
        }
        let dims: Vec<usize> = Array::from(&Reflect::get(&reply, &"dims".into()).map_err(e)?)
            .iter().map(|d| d.as_f64().unwrap_or(0.0) as usize).collect();
        Ok((Float32Array::new(&data).to_vec(), dims))
    }
}

async fn fetch_text(url: &str) -> Result<String, String> {
    let win = web_sys::window().ok_or("no window")?;
    let r: web_sys::Response = JsFuture::from(win.fetch_with_str(url)).await
        .map_err(|_| "The recognizer could not be downloaded. Connect to the internet once, then it works offline".to_string())?
        .dyn_into().map_err(e)?;
    if !r.ok() {
        return Err(format!("The recognizer could not be downloaded ({})", r.status()));
    }
    Ok(JsFuture::from(r.text().map_err(e)?).await.map_err(e)?.as_string().unwrap_or_default())
}

pub async fn sleep(ms: i32) {
    let p = Promise::new(&mut |ok, _| {
        if let Some(w) = web_sys::window() {
            let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(&ok, ms);
        }
    });
    let _ = JsFuture::from(p).await;
}

/// Lets the browser paint progress between heavy steps.
async fn breathe() {
    sleep(0).await;
}

pub async fn recognize(gray: Gray, progress: impl Fn(String)) -> Result<Vec<Line>, String> {
    progress("Preparing the photo…".into());
    breathe().await;
    let ocr = Ocr::get().await?;
    let page = prepare(gray);
    progress("Finding text…".into());
    breathe().await;
    let (prob, dims) = ocr.run("det", Some(&ocr::det_input(&page))).await?;
    if dims.len() != 4 {
        return Err("Unexpected detector output".into());
    }
    let boxes = ocr::det_boxes(&prob, dims[2], dims[3], page.w, page.h);
    let mut read = Vec::with_capacity(boxes.len());
    for (i, b) in boxes.iter().enumerate() {
        if i % 4 == 0 {
            progress(format!("Reading line {} of {}…", i + 1, boxes.len()));
        }
        let Some(t) = ocr::rec_input(&page, b) else { continue };
        let (out, d) = ocr.run("rec", Some(&t)).await?;
        if d.len() == 3 {
            let (text, conf) = ocr::ctc_decode(&out, d[2], &ocr.dict);
            read.push((*b, text, conf));
        }
    }
    Ok(ocr::assemble(read))
}

/// Prints recognized lines only when `localStorage.mmDebug` is "1", for tuning on a test device.
/// Without the flag nothing about a receipt reaches the console.
fn debug_lines(lines: &[Line]) {
    let on = web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item("mmDebug").ok().flatten())
        .is_some_and(|v| v == "1");
    if on {
        for l in lines {
            web_sys::console::debug_1(&format!("ocr {:.2} {}", l.confidence, l.text).into());
        }
    }
}

/// Draws a source into a canvas no larger than `MAX_SIDE` and returns its gray pixels.
fn pixels(source: &JsValue, w: u32, h: u32) -> Result<Gray, String> {
    if w == 0 || h == 0 {
        return Err("The image is empty".into());
    }
    let scale = (MAX_SIDE as f64 / w.max(h) as f64).min(1.0);
    let (cw, ch) = (((w as f64) * scale) as u32, ((h as f64) * scale) as u32);
    let doc = web_sys::window().and_then(|w| w.document()).ok_or("no document")?;
    let canvas: HtmlCanvasElement = doc.create_element("canvas").map_err(e)?.dyn_into().map_err(|_| "no canvas")?;
    canvas.set_width(cw);
    canvas.set_height(ch);
    let ctx: CanvasRenderingContext2d = canvas.get_context("2d").map_err(e)?.ok_or("no 2d context")?.dyn_into().map_err(|_| "no 2d context")?;
    let draw: Function = Reflect::get(&ctx, &"drawImage".into()).map_err(e)?.dyn_into().map_err(|_| "no drawImage")?;
    let args = Array::of5(source, &0.into(), &0.into(), &cw.into(), &ch.into());
    draw.apply(&ctx, &args).map_err(e)?;
    let data = ctx.get_image_data(0.0, 0.0, cw as f64, ch as f64).map_err(e)?;
    let gray = Gray::from_rgba(&data.data(), cw as usize, ch as usize).ok_or("bad image data")?;
    // The canvas held the photo, so it is cleared right away.
    canvas.set_width(0);
    canvas.set_height(0);
    Ok(gray)
}

async fn from_file(input: &web_sys::HtmlInputElement) -> Result<Option<Gray>, String> {
    let Some(file) = input.files().and_then(|f| f.get(0)) else { return Ok(None) };
    let win = web_sys::window().ok_or("no window")?;
    let opts = Object::new();
    let _ = Reflect::set(&opts, &"imageOrientation".into(), &"from-image".into());
    let create: Function = Reflect::get(&win, &"createImageBitmap".into()).map_err(e)?.dyn_into().map_err(|_| "no createImageBitmap")?;
    let p: Promise = create.call2(&win, &file, &opts).map_err(e)?.dyn_into().map_err(|_| "no promise")?;
    let bitmap: web_sys::ImageBitmap = JsFuture::from(p).await.map_err(|_| "This file is not an image the browser can read".to_string())?.dyn_into().map_err(|_| "no bitmap")?;
    let g = pixels(&bitmap, bitmap.width(), bitmap.height());
    bitmap.close();
    input.set_value("");
    g.map(Some)
}

fn draft(c: Ctx, p: &receipt::Parsed) -> Result<Draft, String> {
    let s = c.session.get_untracked().ok_or("The app is locked")?;
    let d = c.data.get_untracked();
    let rules = d.rule_list();
    let merchant = p.merchant.clone().unwrap_or_default();
    let mid = (!merchant.is_empty()).then(|| merchant_id(&s.dek, &merchant));
    let items: Vec<LineItem> = p.items.iter().map(|i| {
        let name = tidy_name(&i.raw_name);
        LineItem {
            raw_name: i.raw_name.clone(),
            category_id: categorize(&rules, mid, &merchant, &name),
            name,
            qty: i.qty,
            unit: i.unit.clone(),
            unit_price: i.unit_price,
            discount: i.discount,
            total: i.value - i.discount,
            vat_rate: i.ptu.and_then(|l| p.vat.iter().find(|v| v.letter == l)).and_then(|v| v.rate_bp),
        }
    }).collect();
    let suggested = items.iter().map(|i| i.category_id).collect();
    let mut counts: HashMap<uuid::Uuid, i64> = HashMap::new();
    for i in &items {
        *counts.entry(i.category_id).or_default() += i.total;
    }
    let category_id = counts.into_iter().max_by_key(|x| (x.1, x.0)).map(|x| x.0)
        .unwrap_or_else(|| categorize(&rules, mid, &merchant, ""));
    let total = p.total.unwrap_or_else(|| p.items_total()).max(0);
    let doubts = Doubts {
        merchant: p.doubts.contains(&Doubt::Merchant),
        nip: p.doubts.contains(&Doubt::Nip) && p.nip.is_some(),
        date: p.doubts.contains(&Doubt::Date),
        total: p.doubts.iter().any(|d| matches!(d, Doubt::Total | Doubt::ItemsSum { .. })) || p.doubts.contains(&Doubt::Vat),
        items: (0..items.len()).map(|i| p.doubts.contains(&Doubt::Item(i))).collect(),
    };
    let tx = Transaction {
        kind: TxKind::Expense,
        date: p.date.unwrap_or_else(today),
        time: p.time.clone(),
        merchant_id: mid,
        nip: p.nip.clone().filter(|n| valid_nip(n)),
        amount: total,
        currency: "PLN".into(),
        amount_pln: total,
        payment: p.payment.unwrap_or_default(),
        account_id: d.account_list().first().map(|a| a.0).unwrap_or(ACCOUNT_MAIN),
        category_id,
        source: Source::Receipt,
        review: if p.doubts.is_empty() { Review::Reviewed } else { Review::NeedsReview },
        note: String::new(),
        items,
        mismatch_confirmed: false,
        duplicate_of: None,
        unique_confirmed: false,
    };
    Ok(Draft { tx, merchant, doubts, suggested })
}

/// Recognizes a receipt and opens it for review.
pub async fn open_receipt(c: Ctx, gray: Gray, status: RwSignal<String>) -> Result<(), String> {
    let lines = recognize(gray, move |s| status.set(s)).await?;
    debug_lines(&lines);
    let parsed = receipt::parse(&lines);
    status.set(String::new());
    if parsed.items.is_empty() && parsed.total.is_none() {
        return Err("No receipt was found in this photo. Try again with the whole receipt in the frame and good light".into());
    }
    c.draft.set_value(Some(draft(c, &parsed)?));
    // Setting the screen notifies even when the form is already open, so it is rebuilt with the draft.
    c.screen.set(Screen::Edit(None));
    Ok(())
}

/// What a chosen file is, by its type and name.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileKind {
    Photo,
    Table,
    Pdf,
    Unknown,
}

pub fn file_kind(mime: &str, name: &str) -> FileKind {
    let ext = name.rsplit_once('.').map(|x| x.1.to_ascii_lowercase()).unwrap_or_default();
    if mime.starts_with("image/") || ["jpg", "jpeg", "png", "heic", "heif", "webp"].contains(&ext.as_str()) {
        FileKind::Photo
    } else if mime == "application/pdf" || ext == "pdf" {
        FileKind::Pdf
    } else if mime.starts_with("text/") || ["csv", "txt", "tsv"].contains(&ext.as_str()) {
        FileKind::Table
    } else {
        FileKind::Unknown
    }
}

/// A file from the Add screen: a receipt photo is recognized, a statement opens the import preview.
pub fn open_upload(c: Ctx, input: web_sys::HtmlInputElement, status: RwSignal<String>) {
    let Some(file) = input.files().and_then(|f| f.get(0)) else { return };
    c.error.set(String::new());
    match file_kind(&file.type_(), &file.name()) {
        FileKind::Photo => crate::app::busy_task(c, async move {
            let g = from_file(&input).await?;
            match g {
                Some(g) => open_receipt(c, g, status).await,
                None => Ok(()),
            }
        }),
        FileKind::Table => spawn_local(async move {
            match JsFuture::from(file.array_buffer()).await {
                Ok(buf) => {
                    c.upload.set_value(Some((file.name(), js_sys::Uint8Array::new(&buf).to_vec())));
                    c.screen.set(Screen::Import);
                }
                Err(_) => c.error.set("The file could not be read".into()),
            }
            input.set_value("");
        }),
        FileKind::Pdf => {
            input.set_value("");
            c.error.set("PDF statements and invoices are not supported yet. Export the statement as CSV in your bank's app, or photograph a receipt".into());
        }
        FileKind::Unknown => {
            input.set_value("");
            c.error.set("Choose a receipt photo, a PDF or a CSV statement".into());
        }
    }
}

fn stop(stream: &Option<MediaStream>) {
    if let Some(s) = stream {
        for t in s.get_tracks().iter() {
            if let Ok(t) = t.dyn_into::<web_sys::MediaStreamTrack>() {
                t.stop();
            }
        }
    }
}

/// Calls a method of a JS object by name, for APIs that web-sys does not cover well.
fn call(o: &JsValue, name: &str, args: &Array) -> Option<JsValue> {
    let f = Reflect::get(o, &name.into()).ok()?.dyn_into::<Function>().ok()?;
    f.apply(o, args).ok()
}

fn video_track(stream: &Option<MediaStream>) -> Option<web_sys::MediaStreamTrack> {
    stream.as_ref()?.get_video_tracks().get(0).dyn_into().ok()
}

/// Turns the phone's light on or off. Browsers without torch support are told by the caller to hide the button.
async fn set_torch(track: web_sys::MediaStreamTrack, on: bool) -> Result<(), String> {
    let adv = Object::new();
    let _ = Reflect::set(&adv, &"torch".into(), &on.into());
    let c = Object::new();
    let _ = Reflect::set(&c, &"advanced".into(), &Array::of1(&adv));
    let p = call(&track, "applyConstraints", &Array::of1(&c)).ok_or("The light cannot be switched here")?;
    JsFuture::from(p.dyn_into::<Promise>().map_err(|_| "no promise")?).await.map(|_| ())
        .map_err(|_| "The light cannot be switched on this phone".to_string())
}

/// Full-screen camera: the shutter, the light and the gallery are within thumb reach.
pub fn scan(c: Ctx) -> impl IntoView {
    let status = RwSignal::new(String::new());
    let camera = RwSignal::new(false);
    let torch_ok = RwSignal::new(false);
    let torch = RwSignal::new(false);
    let stream: StoredValue<Option<MediaStream>, LocalStorage> = StoredValue::new_local(None);
    let video = NodeRef::<leptos::html::Video>::new();

    let run = move |gray: Gray| {
        crate::app::busy_task(c, async move {
            stream.with_value(stop);
            camera.set(false);
            torch.set(false);
            open_receipt(c, gray, status).await
        });
    };

    Effect::new(move |_| {
        let Some(el) = video.get() else { return };
        spawn_local(async move {
            let Some(devices) = web_sys::window().and_then(|w| w.navigator().media_devices().ok()) else { return };
            let c = web_sys::MediaStreamConstraints::new();
            let v = Object::new();
            let _ = Reflect::set(&v, &"facingMode".into(), &"environment".into());
            let _ = Reflect::set(&v, &"width".into(), &js_sys::JSON::parse("{\"ideal\":2560}").unwrap_or(JsValue::NULL));
            c.set_video(&v);
            let Ok(p) = devices.get_user_media_with_constraints(&c) else { return };
            let Ok(s) = JsFuture::from(p).await else { return };
            let Ok(s) = s.dyn_into::<MediaStream>() else { return };
            let node: &HtmlVideoElement = &el;
            node.set_src_object(Some(&s));
            stream.set_value(Some(s));
            if let Some(track) = stream.with_value(video_track) {
                let caps = call(&track, "getCapabilities", &Array::new()).unwrap_or(JsValue::UNDEFINED);
                torch_ok.set(Reflect::get(&caps, &"torch".into()).ok().and_then(|t| t.as_bool()).unwrap_or(false));
            }
            // The shutter is enabled only once frames arrive, otherwise a quick tap captures nothing.
            if let Ok(p) = node.play() {
                let _ = JsFuture::from(p).await;
            }
            // Slow phones can take a few seconds to deliver the first frame.
            for _ in 0..50 {
                if node.video_width() > 0 {
                    camera.set(true);
                    return;
                }
                sleep(100).await;
            }
        });
    });
    on_cleanup(move || stream.with_value(stop));

    let shoot = move |_| {
        let Some(el) = video.get_untracked() else { return };
        let node: &HtmlVideoElement = &el;
        match pixels(node, node.video_width(), node.video_height()) {
            Ok(g) => run(g),
            Err(err) => c.error.set(err),
        }
    };
    let pick = move |ev: leptos::ev::Event| {
        let Some(input) = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) else { return };
        spawn_local(async move {
            match from_file(&input).await {
                Ok(Some(g)) => run(g),
                Ok(None) => {}
                Err(err) => c.error.set(err),
            }
        });
    };
    let flip_torch = move |_| {
        let Some(track) = stream.with_value(video_track) else { return };
        let on = !torch.get_untracked();
        spawn_local(async move {
            match set_torch(track, on).await {
                Ok(()) => torch.set(on),
                Err(e) => c.error.set(e),
            }
        });
    };
    let close = move |_| {
        c.error.set(String::new());
        c.screen.set(Screen::Dashboard);
    };

    view! {
        <section class="camera" class:live=move || camera.get() data-test="camera">
            <video node_ref=video autoplay=true playsinline=true muted=true data-test="scan-video"></video>
            <div class="cam-top">
                <button class="cam-round" data-test="scan-close" aria-label="Close" on:click=close>{icon(Icon::Close)}</button>
                <button class="cam-round" class:on=move || torch.get() data-test="torch" aria-label="Light"
                    aria-pressed=move || torch.get().to_string() hidden=move || !torch_ok.get() on:click=flip_torch>{icon(Icon::Flash)}</button>
            </div>
            <div class="cam-middle">
                {move || (!camera.get() && !c.busy.get()).then(|| view! {
                    <p data-test="no-camera">"The camera is not available. Choose a photo instead."</p>
                })}
                <p data-test="scan-status">{move || status.get()}</p>
                <p class="cam-error">{move || c.error.get()}</p>
            </div>
            <div class="cam-bottom">
                <label class="cam-round" class:disabled=move || c.busy.get() aria-label="Choose a photo">
                    {icon(Icon::Gallery)}
                    <input type="file" accept="image/*" data-test="scan-file" class="file" on:change=pick disabled=move || c.busy.get() />
                </label>
                <button class="shutter" data-test="shutter" aria-label="Take photo" disabled=move || !camera.get() || c.busy.get() on:click=shoot></button>
                <button class="cam-round" data-test="scan-manual" aria-label="Enter by hand" on:click=move |_| c.screen.set(Screen::Edit(None))>{icon(Icon::Pencil)}</button>
            </div>
            <p class="cam-note">"Recognition runs on this phone. The photo is never stored or sent."</p>
        </section>
    }
}
