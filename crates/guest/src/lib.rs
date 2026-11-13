#![cfg(target_os = "android")]
mod transport;
use jni::{
    JNIEnv, JavaVM,
    objects::{GlobalRef, JClass, JObject, JObjectArray, JString, JValue},
    sys::jobjectArray,
};
use std::{
    ffi::c_void,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicI32, AtomicI64, Ordering},
    },
    time::{Duration, Instant},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
#[derive(Default)]
struct Slot {
    display: Option<GlobalRef>,
    reader: Option<GlobalRef>,
    image: Option<GlobalRef>,
}
struct State {
    vm: JavaVM,
    class: GlobalRef,
    manager: GlobalRef,
    handler: GlobalRef,
    input: GlobalRef,
    slots: Mutex<[Slot; 2]>,
    display_ids: [AtomicI32; 2],
    down: [AtomicI64; 2],
    reply: Mutex<(i32, i32)>,
    wake: Condvar,
    token: AtomicI32,
    frames: (Mutex<u64>, Condvar),
}
static STATE: OnceLock<Arc<State>> = OnceLock::new();
fn object<'a>(
    env: &mut JNIEnv<'a>,
    target: &JObject,
    method: &str,
    sig: &str,
    args: &[JValue],
) -> jni::errors::Result<JObject<'a>> {
    env.call_method(target, method, sig, args)?.l()
}
fn static_object<'a>(
    env: &mut JNIEnv<'a>,
    class: &str,
    method: &str,
    sig: &str,
    args: &[JValue],
) -> jni::errors::Result<JObject<'a>> {
    env.call_static_method(class, method, sig, args)?.l()
}
fn string(env: &mut JNIEnv, object: JObject) -> Result<String> {
    if object.is_null() {
        return Ok(String::new());
    }
    let value: JString = object.into();
    Ok(env.get_string(&value)?.into())
}
fn line(message: &str) {
    println!("{message}");
    let _ = std::io::stdout().flush();
}
fn exception(env: &mut JNIEnv, error: impl std::fmt::Display) {
    eprintln!("Rust Android guest: {error}");
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
}
fn context<'a>(env: &mut JNIEnv<'a>) -> Result<JObject<'a>> {
    env.call_static_method("android/os/Looper", "prepareMainLooper", "()V", &[])?;
    let thread = static_object(
        env,
        "android/app/ActivityThread",
        "systemMain",
        "()Landroid/app/ActivityThread;",
        &[],
    )?;
    let system = object(
        env,
        &thread,
        "getSystemContext",
        "()Landroid/app/ContextImpl;",
        &[],
    )?;
    let package = env.new_string("com.android.shell")?;
    Ok(object(
        env,
        &system,
        "createPackageContext",
        "(Ljava/lang/String;I)Landroid/content/Context;",
        &[JValue::Object(&package), JValue::Int(0)],
    )?)
}
fn ensure(env: &mut JNIEnv, state: &State, slot: usize) -> Result<i32> {
    if slot >= 2 {
        return Err("Invalid slot".into());
    }
    if state.slots.lock().unwrap()[slot].display.is_some() {
        return Ok(state.display_ids[slot].load(Ordering::Acquire));
    }
    let reader = static_object(
        env,
        "android/media/ImageReader",
        "newInstance",
        "(IIIIJ)Landroid/media/ImageReader;",
        &[
            540.into(),
            960.into(),
            1.into(),
            3.into(),
            JValue::Long(0x300),
        ],
    )?;
    let class: &JClass = state.class.as_obj().into();
    let listener = env.new_object(class, "(II)V", &[JValue::Int(slot as i32), JValue::Int(-1)])?;
    env.call_method(
        &reader,
        "setOnImageAvailableListener",
        "(Landroid/media/ImageReader$OnImageAvailableListener;Landroid/os/Handler;)V",
        &[
            JValue::Object(&listener),
            JValue::Object(state.handler.as_obj()),
        ],
    )?;
    let surface = object(env, &reader, "getSurface", "()Landroid/view/Surface;", &[])?;
    let name = env.new_string(format!("Rust Android Game Display {}", slot + 1))?;
    let display = object(
        env,
        state.manager.as_obj(),
        "createVirtualDisplay",
        "(Ljava/lang/String;IIILandroid/view/Surface;I)Landroid/hardware/display/VirtualDisplay;",
        &[
            JValue::Object(&name),
            540.into(),
            960.into(),
            180.into(),
            JValue::Object(&surface),
            11.into(),
        ],
    )?;
    let device = object(env, &display, "getDisplay", "()Landroid/view/Display;", &[])?;
    let id = env.call_method(&device, "getDisplayId", "()I", &[])?.i()?;
    let mut slots = state.slots.lock().unwrap();
    slots[slot].display = Some(env.new_global_ref(display)?);
    slots[slot].reader = Some(env.new_global_ref(reader)?);
    state.display_ids[slot].store(id, Ordering::Release);
    line(&format!("NATIVE_DISPLAY_SLOT={slot} ID={id}"));
    Ok(id)
}
fn close(env: &mut JNIEnv, state: &State, slot: usize) -> Result<i32> {
    if slot >= 2 {
        return Err("Invalid slot".into());
    }
    let display = state.slots.lock().unwrap()[slot].display.take();
    if let Some(display) = display {
        env.call_method(display.as_obj(), "release", "()V", &[])?;
    }
    let mut slots = state.slots.lock().unwrap();
    if let Some(image) = slots[slot].image.take() {
        env.call_method(image.as_obj(), "close", "()V", &[])?;
    }
    if let Some(reader) = slots[slot].reader.take() {
        env.call_method(reader.as_obj(), "close", "()V", &[])?;
    }
    state.display_ids[slot].store(-1, Ordering::Release);
    line(&format!("NATIVE_DISPLAY_CLOSED_SLOT={slot}"));
    Ok(0)
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_MinimalBootstrap_onImageAvailable(
    mut env: JNIEnv,
    this: JObject,
    reader: JObject,
) {
    let result = (|| -> Result<()> {
        let state = STATE.get().ok_or("Guest is not initialized")?;
        let slot = env.get_field(&this, "slot", "I")?.i()? as usize;
        if slot >= 2 {
            return Ok(());
        }
        let image = object(
            &mut env,
            &reader,
            "acquireLatestImage",
            "()Landroid/media/Image;",
            &[],
        )?;
        if image.is_null() {
            return Ok(());
        }
        let mut slots = state.slots.lock().unwrap();
        if slots[slot].display.is_none() {
            env.call_method(&image, "close", "()V", &[])?;
            return Ok(());
        }
        if let Some(previous) = slots[slot].image.take() {
            env.call_method(previous.as_obj(), "close", "()V", &[])?;
        }
        slots[slot].image = Some(env.new_global_ref(image)?);
        drop(slots);
        *state.frames.0.lock().unwrap() += 1;
        state.frames.1.notify_one();
        Ok(())
    })();
    if let Err(error) = result {
        exception(&mut env, error);
    }
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_MinimalBootstrap_run(mut env: JNIEnv, this: JObject) {
    let Some(state) = STATE.get() else { return };
    let token = env
        .get_field(&this, "slot", "I")
        .and_then(|v| v.i())
        .unwrap_or(-1);
    let command = env
        .get_field(&this, "command", "I")
        .and_then(|v| v.i())
        .unwrap_or(i32::MIN);
    let result = if command >= 0 {
        ensure(&mut env, state, command as usize)
    } else {
        close(&mut env, state, (-command - 1) as usize)
    };
    let id = match result {
        Ok(id) => id,
        Err(error) => {
            exception(&mut env, error);
            -1
        }
    };
    *state.reply.lock().unwrap() = (token, id);
    state.wake.notify_all();
}
fn read_int(stream: &mut TcpStream) -> std::io::Result<u32> {
    let mut bytes = [0; 4];
    stream.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}
fn control(state: Arc<State>) {
    let Ok(listener) = TcpListener::bind("127.0.0.1:9168") else {
        return;
    };
    for client in listener.incoming() {
        let result = (|| -> Result<()> {
            let mut stream = client?;
            stream.set_read_timeout(Some(Duration::from_secs(5)))?;
            let command = read_int(&mut stream)? as i32;
            let slot = if command < 0 { -command - 1 } else { command };
            if !(0..=1).contains(&slot) {
                return Err("Invalid slot".into());
            }
            let mut env = state.vm.attach_current_thread()?;
            let class: &JClass = state.class.as_obj().into();
            let token = state.token.fetch_add(1, Ordering::Relaxed);
            let runnable =
                env.new_object(class, "(II)V", &[JValue::Int(token), JValue::Int(command)])?;
            env.call_method(
                state.handler.as_obj(),
                "post",
                "(Ljava/lang/Runnable;)Z",
                &[JValue::Object(&runnable)],
            )?;
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut reply = state.reply.lock().unwrap();
            while reply.0 != token && Instant::now() < deadline {
                let duration = deadline.saturating_duration_since(Instant::now());
                reply = state
                    .wake
                    .wait_timeout(reply, duration)
                    .map_err(|_| std::io::Error::other("Guest control lock poisoned"))?
                    .0;
            }
            let id = if reply.0 == token { reply.1 } else { -1 };
            stream.write_all(&id.to_le_bytes())?;
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("Rust display control: {error}");
        }
    }
}
#[link(name = "android")]
unsafe extern "C" {
    fn AHardwareBuffer_fromHardwareBuffer(
        env: *mut jni::sys::JNIEnv,
        buffer: jni::sys::jobject,
    ) -> *mut c_void;
}
#[link(name = "dl")]
unsafe extern "C" {
    fn dlopen(name: *const std::ffi::c_char, flags: i32) -> *mut c_void;
    fn dlsym(module: *mut c_void, symbol: *const std::ffi::c_char) -> *mut c_void;
}
fn color_buffer(env: &JNIEnv, hardware: &JObject) -> u32 {
    type NativeHandle = unsafe extern "C" fn(*mut c_void) -> *const i32;
    static GET: OnceLock<usize> = OnceLock::new();
    unsafe {
        let buffer = AHardwareBuffer_fromHardwareBuffer(env.get_raw(), hardware.as_raw());
        if buffer.is_null() {
            return 0;
        }
        let pointer = *GET.get_or_init(|| {
            let module = dlopen(c"libnativewindow.so".as_ptr(), 2);
            if module.is_null() {
                return 0;
            }
            dlsym(module, c"AHardwareBuffer_getNativeHandle".as_ptr()) as usize
        });
        if pointer == 0 {
            return 0;
        }
        let get: NativeHandle = std::mem::transmute(pointer);
        let words = get(buffer);
        if words.is_null()
            || *words != 12
            || *words.add(1) != 1
            || *words.add(2) != 20
            || *words.add(5) as u32 != 0xabfabfa1
        {
            return 0;
        }
        *words.add(6) as u32
    }
}
fn inject(env: &mut JNIEnv, state: &State, input: [u32; 7]) -> Result<()> {
    let kind = input[1];
    let slot = input[2] as usize;
    if slot >= 2 || kind == 0 {
        return Ok(());
    }
    if kind == 3 {
        return Ok(());
    }
    let now = env
        .call_static_method("android/os/SystemClock", "uptimeMillis", "()J", &[])?
        .j()?;
    let display = state.display_ids[slot].load(Ordering::Acquire);
    if display < 0 {
        return Ok(());
    }
    if kind == 1 {
        let action = input[3] as i32;
        if action == 0 {
            state.down[slot].store(now, Ordering::Relaxed);
        }
        let event = static_object(
            env,
            "android/view/MotionEvent",
            "obtain",
            "(JJIFFI)Landroid/view/MotionEvent;",
            &[
                JValue::Long(state.down[slot].load(Ordering::Relaxed)),
                JValue::Long(now),
                action.into(),
                JValue::Float(input[4] as f32),
                JValue::Float(input[5] as f32),
                0.into(),
            ],
        )?;
        env.call_method(&event, "setSource", "(I)V", &[0x1002.into()])?;
        env.call_method(&event, "setDisplayId", "(I)V", &[display.into()])?;
        let accepted = env
            .call_method(
                state.input.as_obj(),
                "injectInputEvent",
                "(Landroid/view/InputEvent;I)Z",
                &[JValue::Object(&event), 1.into()],
            )?
            .z()?;
        env.call_method(&event, "recycle", "()V", &[])?;
        if !accepted && action != 3 {
            return Err(
                format!("Android rejected touch action {action} on display {display}").into(),
            );
        }
    } else if kind == 2 {
        for action in [0, 1] {
            let event = env.new_object(
                "android/view/KeyEvent",
                "(JJIII)V",
                &[
                    JValue::Long(now),
                    JValue::Long(now),
                    action.into(),
                    JValue::Int(input[6] as i32),
                    0.into(),
                ],
            )?;
            env.call_method(&event, "setDisplayId", "(I)V", &[display.into()])?;
            env.call_method(
                state.input.as_obj(),
                "injectInputEvent",
                "(Landroid/view/InputEvent;I)Z",
                &[JValue::Object(&event), 0.into()],
            )?;
        }
    }
    Ok(())
}
fn input_server(state: Arc<State>) {
    let Ok(listener) = TcpListener::bind("127.0.0.1:9167") else {
        return;
    };
    line("NATIVE_INPUT_READY");
    for client in listener.incoming() {
        let result = (|| -> Result<()> {
            let mut socket = client?;
            socket.set_nodelay(true)?;
            socket.set_read_timeout(Some(Duration::from_secs(5)))?;
            socket.set_write_timeout(Some(Duration::from_secs(2)))?;
            let mut env = state.vm.attach_current_thread()?;
            loop {
                let mut bytes = [0; 32];
                socket.read_exact(&mut bytes)?;
                let packet: Vec<u32> = bytes
                    .chunks_exact(4)
                    .map(|value| u32::from_le_bytes(value.try_into().unwrap()))
                    .collect();
                if packet[0] != 0x42535449 || !(1..=3).contains(&packet[1]) || packet[2] >= 2 {
                    return Err("Invalid input packet".into());
                }
                env.with_local_frame(32, |env| -> Result<()> {
                    inject(env, &state, packet[..7].try_into().unwrap())
                })?;
                let ack = [0x42535441, packet[7]]
                    .into_iter()
                    .flat_map(u32::to_le_bytes)
                    .collect::<Vec<_>>();
                socket.write_all(&ack)?;
            }
        })();
        if let Err(error) = result {
            eprintln!("Rust input channel: {error}");
        }
    }
}
fn image_ready(env: &mut JNIEnv, image: &JObject) -> Result<bool> {
    let fence = object(
        env,
        image,
        "getFence",
        "()Landroid/hardware/SyncFence;",
        &[],
    )?;
    if fence.is_null() {
        return Ok(true);
    }
    let result = (|| -> Result<bool> {
        if !env.call_method(&fence, "isValid", "()Z", &[])?.z()? {
            return Ok(true);
        }
        let timeout = static_object(
            env,
            "java/time/Duration",
            "ofMillis",
            "(J)Ljava/time/Duration;",
            &[JValue::Long(100)],
        )?;
        Ok(env
            .call_method(
                &fence,
                "await",
                "(Ljava/time/Duration;)Z",
                &[JValue::Object(&timeout)],
            )?
            .z()?)
    })();
    let closed = env.call_method(&fence, "close", "()V", &[]);
    let ready = result?;
    closed?;
    Ok(ready)
}
fn broker(state: Arc<State>) {
    let Ok(listener) = TcpListener::bind("127.0.0.1:9166") else {
        return;
    };
    line("NATIVE_BUFFER_BROKER_READY");
    for client in listener.incoming() {
        let result = (|| -> Result<()> {
            let mut socket = client?;
            socket.set_read_timeout(Some(Duration::from_secs(5)))?;
            socket.set_write_timeout(Some(Duration::from_secs(5)))?;
            socket.set_nodelay(true)?;
            let mut env = state.vm.attach_current_thread()?;
            let mut sequence = 0u32;
            let mut seen = 0;
            let mut timing = Instant::now();
            let mut samples = [0u64; 2];
            let mut waits = [0u128; 2];
            let mut ages = [0u128; 2];
            loop {
                let mut revision = state.frames.0.lock().unwrap();
                if *revision == seen {
                    revision = state
                        .frames
                        .1
                        .wait_timeout_while(revision, Duration::from_millis(500), |revision| {
                            *revision == seen
                        })
                        .unwrap()
                        .0;
                }
                let idle = seen == *revision;
                seen = *revision;
                drop(revision);
                if idle {
                    transport::check_receiver(&socket)?;
                }
                for slot in 0..2 {
                    let result = env.with_local_frame(48, |env| -> Result<()> {
                        let image = state.slots.lock().unwrap()[slot].image.take();
                        let Some(image) = image else {
                            return Ok(());
                        };
                        let result = (|| -> Result<()> {
                            let started = Instant::now();
                            if !image_ready(env, image.as_obj())? {
                                eprintln!(
                                    "GPU fence timed out for slot {slot}; skipped incomplete frame"
                                );
                                return Ok(());
                            }
                            samples[slot] += 1;
                            waits[slot] += started.elapsed().as_micros();
                            let timestamp = env
                                .call_method(image.as_obj(), "getTimestamp", "()J", &[])?
                                .j()?;
                            let now = env
                                .call_static_method("java/lang/System", "nanoTime", "()J", &[])?
                                .j()?;
                            ages[slot] += now.saturating_sub(timestamp).max(0) as u128 / 1000;
                            let hardware = object(
                                env,
                                image.as_obj(),
                                "getHardwareBuffer",
                                "()Landroid/hardware/HardwareBuffer;",
                                &[],
                            )?;
                            if hardware.is_null() {
                                return Ok(());
                            }
                            let result = (|| -> Result<()> {
                                let buffer = color_buffer(env, &hardware);
                                if buffer != 0 {
                                    let width =
                                        env.call_method(image.as_obj(), "getWidth", "()I", &[])?
                                            .i()? as u32;
                                    let height =
                                        env.call_method(image.as_obj(), "getHeight", "()I", &[])?
                                            .i()? as u32;
                                    sequence = sequence.wrapping_add(1);
                                    let header =
                                        [0x42535446, slot as u32, buffer, width, height, sequence]
                                            .into_iter()
                                            .flat_map(u32::to_le_bytes)
                                            .collect::<Vec<_>>();
                                    socket.write_all(&header)?;
                                    let mut input = [0; 7];
                                    for value in &mut input {
                                        *value = read_int(&mut socket)?;
                                    }
                                    if input[0] != 0x42535449 {
                                        return Err("Invalid host acknowledgement".into());
                                    }
                                }
                                Ok(())
                            })();
                            let close = env.call_method(&hardware, "close", "()V", &[]);
                            result?;
                            close?;
                            Ok(())
                        })();
                        let close = env.call_method(image.as_obj(), "close", "()V", &[]);
                        result?;
                        close?;
                        Ok(())
                    });
                    if let Err(error) = result {
                        exception(&mut env, &error);
                        return Err(error);
                    }
                }
                if timing.elapsed() >= Duration::from_secs(5) {
                    for slot in 0..2 {
                        if samples[slot] > 0 {
                            line(&format!(
                                "NATIVE_FRAME_TIMING slot={slot} samples={} fence_wait_avg_ms={:.2} frame_age_avg_ms={:.2}",
                                samples[slot],
                                waits[slot] as f64 / samples[slot] as f64 / 1000.,
                                ages[slot] as f64 / samples[slot] as f64 / 1000.
                            ));
                        }
                    }
                    samples = [0; 2];
                    waits = [0; 2];
                    ages = [0; 2];
                    timing = Instant::now();
                }
            }
        })();
        if let Err(error) = result {
            eprintln!("Rust display broker: {error}");
        }
    }
}
fn icon(env: &mut JNIEnv, pm: &JObject, app: &JObject, package: &str) -> Result<String> {
    let name = format!("{package}.png");
    let drawable = object(
        env,
        pm,
        "getApplicationIcon",
        "(Landroid/content/pm/ApplicationInfo;)Landroid/graphics/drawable/Drawable;",
        &[JValue::Object(app)],
    )?;
    let config = env
        .get_static_field(
            "android/graphics/Bitmap$Config",
            "ARGB_8888",
            "Landroid/graphics/Bitmap$Config;",
        )?
        .l()?;
    let bitmap = static_object(
        env,
        "android/graphics/Bitmap",
        "createBitmap",
        "(IILandroid/graphics/Bitmap$Config;)Landroid/graphics/Bitmap;",
        &[256.into(), 256.into(), JValue::Object(&config)],
    )?;
    let canvas = env.new_object(
        "android/graphics/Canvas",
        "(Landroid/graphics/Bitmap;)V",
        &[JValue::Object(&bitmap)],
    )?;
    env.call_method(
        &drawable,
        "setBounds",
        "(IIII)V",
        &[0.into(), 0.into(), 256.into(), 256.into()],
    )?;
    env.call_method(
        &drawable,
        "draw",
        "(Landroid/graphics/Canvas;)V",
        &[JValue::Object(&canvas)],
    )?;
    let file = env.new_string(format!("/data/local/tmp/minimal-app-icons/{name}"))?;
    let stream = env.new_object(
        "java/io/FileOutputStream",
        "(Ljava/lang/String;)V",
        &[JValue::Object(&file)],
    )?;
    let format = env
        .get_static_field(
            "android/graphics/Bitmap$CompressFormat",
            "PNG",
            "Landroid/graphics/Bitmap$CompressFormat;",
        )?
        .l()?;
    let compressed = env.call_method(
        &bitmap,
        "compress",
        "(Landroid/graphics/Bitmap$CompressFormat;ILjava/io/OutputStream;)Z",
        &[JValue::Object(&format), 100.into(), JValue::Object(&stream)],
    );
    let _ = env.call_method(&stream, "close", "()V", &[]);
    let _ = env.call_method(&bitmap, "recycle", "()V", &[]);
    if !compressed?.z()? {
        return Err("App icon compression failed".into());
    }
    Ok(name)
}
fn inspect(env: &mut JNIEnv, context: &JObject) -> Result<()> {
    let pm = object(
        env,
        context,
        "getPackageManager",
        "()Landroid/content/pm/PackageManager;",
        &[],
    )?;
    let list = object(
        env,
        &pm,
        "getInstalledApplications",
        "(I)Ljava/util/List;",
        &[512.into()],
    )?;
    let size = env.call_method(&list, "size", "()I", &[])?.i()?;
    std::fs::create_dir_all("/data/local/tmp/minimal-app-icons")?;
    let mut rows = Vec::new();
    for index in 0..size {
        rows.push(env.with_local_frame(96,|env|->Result<serde_json::Value>{let app=object(env,&list,"get","(I)Ljava/lang/Object;",&[index.into()])?;let package_value=env.get_field(&app,"packageName","Ljava/lang/String;")?.l()?;let package=string(env,package_value)?;let label=object(env,&pm,"getApplicationLabel","(Landroid/content/pm/ApplicationInfo;)Ljava/lang/CharSequence;",&[JValue::Object(&app)])?;let label=object(env,&label,"toString","()Ljava/lang/String;",&[])?;let name=string(env,label)?;let enabled=env.get_field(&app,"enabled","Z")?.z()?;let flags=env.get_field(&app,"flags","I")?.i()?;let pkg=env.new_string(&package)?;let intent=object(env,&pm,"getLaunchIntentForPackage","(Ljava/lang/String;)Landroid/content/Intent;",&[JValue::Object(&pkg)])?;let activity=if intent.is_null(){String::new()}else{let component=object(env,&intent,"getComponent","()Landroid/content/ComponentName;",&[])?;if component.is_null(){String::new()}else{let text=object(env,&component,"flattenToString","()Ljava/lang/String;",&[])?;string(env,text)?}};let icon=if package.chars().all(|c|c.is_ascii_alphanumeric()||c=='.'||c=='_'){match icon(env,&pm,&app,&package){Ok(icon)=>icon,Err(error)=>{exception(env,error);String::new()}}}else{String::new()};Ok(serde_json::json!({"package":package,"name":name,"activity":activity,"system":flags&1!=0,"enabled":enabled,"icon":icon}))})?);
    }
    line(&serde_json::to_string(&rows)?);
    Ok(())
}
fn main_rust(env: &mut JNIEnv, class: JClass, args: JObjectArray) -> Result<()> {
    let mut argv = Vec::new();
    // Argument zero is the immutable native-library path consumed by the stub.
    for index in 1..env.get_array_length(&args)? {
        let arg = env.get_object_array_element(&args, index)?;
        argv.push(string(env, arg)?);
    }
    if argv
        .first()
        .is_some_and(|s| s == "--ensure-display" || s == "--close-display")
    {
        let slot = argv.get(1).ok_or("Missing slot")?.parse::<i32>()?;
        if !(0..=1).contains(&slot) {
            return Err("Invalid slot".into());
        }
        let close = argv[0] == "--close-display";
        let mut stream = TcpStream::connect("127.0.0.1:9168")?;
        stream.set_read_timeout(Some(Duration::from_secs(15)))?;
        stream.write_all(&(if close { -slot - 1 } else { slot }).to_le_bytes())?;
        let id = read_int(&mut stream)? as i32;
        if id < 0 {
            return Err("Display request failed".into());
        }
        line(&if close {
            format!("NATIVE_DISPLAY_CLOSED_SLOT={slot}")
        } else {
            format!("NATIVE_DISPLAY_SLOT={slot} ID={id}")
        });
        return Ok(());
    }
    let context = context(env)?;
    if argv.first().is_some_and(|s| s == "--inspect") {
        return inspect(env, &context);
    }
    let service = env.new_string("display")?;
    let manager = object(
        env,
        &context,
        "getSystemService",
        "(Ljava/lang/String;)Ljava/lang/Object;",
        &[JValue::Object(&service)],
    )?;
    let input = static_object(
        env,
        "android/hardware/input/InputManager",
        "getInstance",
        "()Landroid/hardware/input/InputManager;",
        &[],
    )?;
    let handler = env.new_object("android/os/Handler", "()V", &[])?;
    let state = Arc::new(State {
        vm: env.get_java_vm()?,
        class: env.new_global_ref(class)?,
        manager: env.new_global_ref(manager)?,
        handler: env.new_global_ref(handler)?,
        input: env.new_global_ref(input)?,
        slots: Mutex::new(std::array::from_fn(|_| Slot::default())),
        display_ids: std::array::from_fn(|_| AtomicI32::new(-1)),
        down: std::array::from_fn(|_| AtomicI64::new(0)),
        reply: Mutex::new((-1, -1)),
        wake: Condvar::new(),
        token: AtomicI32::new(1),
        frames: (Mutex::new(0), Condvar::new()),
    });
    STATE
        .set(state.clone())
        .map_err(|_| "Already initialized")?;
    let slot = argv.first().map_or(Ok(0), |s| s.parse::<usize>())?;
    ensure(env, &state, slot)?;
    let broker_state = state.clone();
    std::thread::spawn(move || broker(broker_state));
    let input_state = state.clone();
    std::thread::spawn(move || input_server(input_state));
    std::thread::spawn(move || control(state));
    env.call_static_method("android/os/Looper", "loop", "()V", &[])?;
    Ok(())
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_MinimalBootstrap_mainRust(
    mut env: JNIEnv,
    class: JClass,
    args: jobjectArray,
) {
    let args = unsafe { JObjectArray::from_raw(args) };
    if let Err(error) = main_rust(&mut env, class, args) {
        exception(&mut env, &error);
        let _ = env.throw_new("java/lang/RuntimeException", error.to_string());
    }
}
