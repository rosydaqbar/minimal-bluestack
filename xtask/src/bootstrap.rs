// ART requires a class with main(). This emits only the ABI entry/callback stubs;
// display creation, scanning, networking and input are all implemented in Rust.
// There is no Java source compiler or embedded legacy Java implementation.
struct Pool(Vec<Vec<u8>>);
fn u16(out: &mut Vec<u8>, n: u16) {
    out.extend(n.to_be_bytes());
}
fn u32(out: &mut Vec<u8>, n: u32) {
    out.extend(n.to_be_bytes());
}
impl Pool {
    fn entry(&mut self, bytes: Vec<u8>) -> u16 {
        self.0.push(bytes);
        self.0.len() as u16
    }
    fn text(&mut self, text: &str) -> u16 {
        let mut out = vec![1];
        u16(&mut out, text.len() as u16);
        out.extend(text.as_bytes());
        self.entry(out)
    }
    fn class(&mut self, name: &str) -> u16 {
        let name = self.text(name);
        let mut out = vec![7];
        u16(&mut out, name);
        self.entry(out)
    }
    fn member(&mut self, tag: u8, class: &str, name: &str, sig: &str) -> u16 {
        let class = self.class(class);
        let name = self.text(name);
        let sig = self.text(sig);
        let mut pair = vec![12];
        u16(&mut pair, name);
        u16(&mut pair, sig);
        let pair = self.entry(pair);
        let mut out = vec![tag];
        u16(&mut out, class);
        u16(&mut out, pair);
        self.entry(out)
    }
}
fn instruction(out: &mut Vec<u8>, opcode: u8, index: u16) {
    out.push(opcode);
    u16(out, index);
}
fn method(
    out: &mut Vec<u8>,
    flags: u16,
    name: u16,
    sig: u16,
    code: Option<(u16, &[u8], u16, u16)>,
) {
    u16(out, flags);
    u16(out, name);
    u16(out, sig);
    if let Some((attribute, bytes, stack, locals)) = code {
        u16(out, 1);
        u16(out, attribute);
        u32(out, (12 + bytes.len()) as u32);
        u16(out, stack);
        u16(out, locals);
        u32(out, bytes.len() as u32);
        out.extend(bytes);
        u16(out, 0);
        u16(out, 0);
    } else {
        u16(out, 0);
    }
}
pub fn class_file() -> Vec<u8> {
    let mut cp = Pool(Vec::new());
    let class = cp.class("MinimalBootstrap");
    let parent = cp.class("java/lang/Object");
    let runnable = cp.class("java/lang/Runnable");
    let listener = cp.class("android/media/ImageReader$OnImageAvailableListener");
    let integer = cp.text("I");
    let slot = cp.text("slot");
    let command = cp.text("command");
    let code = cp.text("Code");
    let init_name = cp.text("<init>");
    let init_sig = cp.text("(II)V");
    let init = cp.member(10, "java/lang/Object", "<init>", "()V");
    let slot_field = cp.member(9, "MinimalBootstrap", "slot", "I");
    let command_field = cp.member(9, "MinimalBootstrap", "command", "I");
    let main_name = cp.text("main");
    let array_sig = cp.text("([Ljava/lang/String;)V");
    let native_name = cp.text("mainRust");
    let native_main = cp.member(10, "MinimalBootstrap", "mainRust", "([Ljava/lang/String;)V");
    let load = cp.member(10, "java/lang/System", "load", "(Ljava/lang/String;)V");
    let run_name = cp.text("run");
    let void_sig = cp.text("()V");
    let image_name = cp.text("onImageAvailable");
    let image_sig = cp.text("(Landroid/media/ImageReader;)V");
    let mut constructor = vec![0x2a];
    instruction(&mut constructor, 0xb7, init);
    constructor.extend([0x2a, 0x1b]);
    instruction(&mut constructor, 0xb5, slot_field);
    constructor.extend([0x2a, 0x1c]);
    instruction(&mut constructor, 0xb5, command_field);
    constructor.push(0xb1);
    let mut main = vec![0x2a, 0x03, 0x32];
    instruction(&mut main, 0xb8, load);
    main.push(0x2a);
    instruction(&mut main, 0xb8, native_main);
    main.push(0xb1);
    let mut out = Vec::new();
    u32(&mut out, 0xcafebabe);
    u16(&mut out, 0);
    u16(&mut out, 52);
    u16(&mut out, cp.0.len() as u16 + 1);
    for item in cp.0 {
        out.extend(item);
    }
    u16(&mut out, 0x31);
    u16(&mut out, class);
    u16(&mut out, parent);
    u16(&mut out, 2);
    u16(&mut out, runnable);
    u16(&mut out, listener);
    u16(&mut out, 2);
    for field in [slot, command] {
        u16(&mut out, 0x11);
        u16(&mut out, field);
        u16(&mut out, integer);
        u16(&mut out, 0);
    }
    u16(&mut out, 5);
    method(
        &mut out,
        1,
        init_name,
        init_sig,
        Some((code, &constructor, 2, 3)),
    );
    method(&mut out, 9, main_name, array_sig, Some((code, &main, 2, 1)));
    method(&mut out, 0x109, native_name, array_sig, None);
    method(&mut out, 0x101, run_name, void_sig, None);
    method(&mut out, 0x101, image_name, image_sig, None);
    u16(&mut out, 0);
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bootstrap_contains_only_native_stubs_and_loader() {
        let bytes = class_file();
        assert_eq!(&bytes[..4], &[0xca, 0xfe, 0xba, 0xbe]);
        assert!(bytes.windows(8).any(|w| w == b"mainRust"));
        assert!(!bytes.windows(20).any(|w| w == b"createVirtualDisplay"));
        assert!(!bytes.windows(20).any(|w| w == b"getInstalledPackages"));
    }
}
