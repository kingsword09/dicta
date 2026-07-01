use std::cell::RefCell;
use std::rc::Rc;

use js_sys::Array;
use wasm_bindgen::{JsCast, JsValue, closure::Closure, prelude::wasm_bindgen};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    Blob, BlobEvent, File, FilePropertyBag, MediaRecorder, MediaRecorderOptions, MediaStream,
    MediaStreamConstraints, MediaStreamTrack,
};

use crate::error;

const RECORDING_MIME_CANDIDATES: &[(&str, &str)] = &[
    ("audio/webm;codecs=opus", "webm"),
    ("audio/webm", "webm"),
    ("audio/mp4", "m4a"),
    ("audio/ogg;codecs=opus", "ogg"),
];

#[wasm_bindgen]
pub fn recommended_recording_mime_type() -> String {
    RECORDING_MIME_CANDIDATES
        .iter()
        .find_map(|(mime, _)| MediaRecorder::is_type_supported(mime).then(|| (*mime).to_owned()))
        .unwrap_or_else(|| "audio/webm".to_owned())
}

#[wasm_bindgen]
pub fn recommended_recording_extension(mime_type: &str) -> String {
    extension_for_mime(mime_type).to_owned()
}

#[wasm_bindgen]
pub async fn record_microphone(seconds: f64) -> Result<File, JsValue> {
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err(error::message(
            "recording duration must be greater than zero",
        ));
    }

    let window = web_sys::window().ok_or_else(|| error::message("window is unavailable"))?;
    let media_devices = window.navigator().media_devices()?;
    let constraints = MediaStreamConstraints::new();
    constraints.set_audio(&JsValue::TRUE);

    let stream = JsFuture::from(media_devices.get_user_media_with_constraints(&constraints)?)
        .await?
        .dyn_into::<MediaStream>()
        .map_err(|_| error::message("getUserMedia did not return a MediaStream"))?;

    let mime_type = recommended_recording_mime_type();
    let options = MediaRecorderOptions::new();
    options.set_mime_type(&mime_type);

    let recorder =
        match MediaRecorder::new_with_media_stream_and_media_recorder_options(&stream, &options) {
            Ok(recorder) => recorder,
            Err(_) => MediaRecorder::new_with_media_stream(&stream)?,
        };
    let recorder_mime_type = non_empty(&recorder.mime_type())
        .map(str::to_owned)
        .unwrap_or(mime_type);
    let file_name = format!("vo-recording.{}", extension_for_mime(&recorder_mime_type));

    let chunks = Rc::new(RefCell::new(Vec::<Blob>::new()));
    let data_chunks = Rc::clone(&chunks);
    let ondataavailable = Closure::<dyn FnMut(BlobEvent)>::new(move |event: BlobEvent| {
        if let Some(blob) = event.data() {
            if blob.size() > 0.0 {
                data_chunks.borrow_mut().push(blob);
            }
        }
    });
    recorder.set_ondataavailable(Some(ondataavailable.as_ref().unchecked_ref()));

    let stop_promise = js_sys::Promise::new(&mut |resolve, reject| {
        let chunks = Rc::clone(&chunks);
        let stop_stream = stream.clone();
        let stop_recorder = recorder.clone();
        let recorder_mime_type = recorder_mime_type.clone();
        let file_name = file_name.clone();
        let resolve_stop = resolve.clone();
        let reject_stop = reject.clone();

        let onstop = Closure::<dyn FnMut()>::once(move || {
            stop_stream_tracks(&stop_stream);
            match file_from_chunks(&chunks.borrow(), &file_name, &recorder_mime_type) {
                Ok(file) => {
                    let _ = resolve_stop.call1(&JsValue::UNDEFINED, &file);
                }
                Err(err) => {
                    let _ = reject_stop.call1(&JsValue::UNDEFINED, &err);
                }
            }
            stop_recorder.set_ondataavailable(None);
            stop_recorder.set_onstop(None);
            stop_recorder.set_onerror(None);
        });
        recorder.set_onstop(Some(onstop.as_ref().unchecked_ref()));
        onstop.forget();

        let error_stream = stream.clone();
        let error_recorder = recorder.clone();
        let onerror = Closure::<dyn FnMut(JsValue)>::once(move |event: JsValue| {
            stop_stream_tracks(&error_stream);
            error_recorder.set_ondataavailable(None);
            error_recorder.set_onstop(None);
            error_recorder.set_onerror(None);
            let _ = reject.call1(&JsValue::UNDEFINED, &event);
        });
        recorder.set_onerror(Some(onerror.as_ref().unchecked_ref()));
        onerror.forget();
    });

    if let Err(err) = recorder.start() {
        stop_stream_tracks(&stream);
        return Err(err);
    }
    let stop_recorder = recorder.clone();
    let timeout = Closure::<dyn FnMut()>::once(move || {
        let _ = stop_recorder.stop();
    });
    if let Err(err) = window.set_timeout_with_callback_and_timeout_and_arguments_0(
        timeout.as_ref().unchecked_ref(),
        (seconds * 1000.0).round() as i32,
    ) {
        let _ = recorder.stop();
        stop_stream_tracks(&stream);
        return Err(err);
    }
    timeout.forget();

    let file = JsFuture::from(stop_promise)
        .await?
        .dyn_into::<File>()
        .map_err(|_| error::message("recording did not produce a File"))?;
    ondataavailable.forget();
    Ok(file)
}

fn file_from_chunks(chunks: &[Blob], file_name: &str, mime_type: &str) -> Result<File, JsValue> {
    if chunks.is_empty() {
        return Err(error::message("recording produced no audio data"));
    }

    let parts = Array::new();
    for chunk in chunks {
        parts.push(chunk);
    }

    let options = FilePropertyBag::new();
    options.set_type(mime_type);
    File::new_with_blob_sequence_and_options(&parts, file_name, &options)
}

fn stop_stream_tracks(stream: &MediaStream) {
    for track in stream.get_tracks().iter() {
        if let Ok(track) = track.dyn_into::<MediaStreamTrack>() {
            track.stop();
        }
    }
}

fn extension_for_mime(mime_type: &str) -> &'static str {
    RECORDING_MIME_CANDIDATES
        .iter()
        .find_map(|(candidate, extension)| {
            mime_type
                .starts_with(candidate.split(';').next().unwrap_or(candidate))
                .then_some(*extension)
        })
        .unwrap_or("webm")
}

fn non_empty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}
