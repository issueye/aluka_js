//! Phase 4 `crypto` 内置库端到端与接口对拍测试：
//! - 摘要：`createHash` / `createHmac` / `hash` / `getHashes`
//! - 对称密码：`createCipheriv` / `createDecipheriv`（CBC/ECB/CTR/GCM）
//! - KDF：`pbkdf2` / `scrypt` / `hkdf`（同步 + 异步回调）
//! - 随机：`randomBytes` / `randomUUID` / `randomInt` / `randomFillSync` / `randomFill`
//! - 其他：`timingSafeEqual` / `createSecretKey` / `checkPrime` / `getCiphers`
//! - X.509：`X509Certificate` / `createPrivateKey`
//!
//! 与 Node.js 22 LTS 规范逐字比对：alukac 前端整图编译 →
//! aluvm 原生驱动执行，输出与 Node.js 22 严格一致。
//!
//! 探针约束（保证输出确定）：随机 API 只断言长度/格式/取值域，不打印随机
//! 值；每个探针至多一个异步回调（Go 侧多异步回调的相对顺序受 goroutine
//! 调度影响，存在竞态）。

mod common;

use std::path::PathBuf;

/// 创建隔离的临时测试目录
fn work_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "builtins_phase4_crypto_{name}_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建工作目录失败");
    dir
}

/// createHash 全算法族（md5/sha1/sha256/sha384/sha512）链式 update/digest 对拍
#[test]
fn hash_create_family_e2e_matches_go() {
    let work = work_dir("p01_hash");
    std::fs::write(
        work.join("probe.js"),
        r#"const Buffer = require("buffer").Buffer;
const crypto = require("crypto");
const algos = ["md5", "sha1", "sha256", "sha384", "sha512"];
for (let i = 0; i < algos.length; i++) {
  const h = crypto.createHash(algos[i]);
  h.update("hello world");
  console.log(algos[i], h.digest("hex"));
}
// chaining + multiple updates
const h1 = crypto.createHash("sha256");
h1.update("hello, ");
h1.update("");
h1.update("chained world");
console.log("chain:", h1.digest("hex"));
// buffer default digest + toString
const h2 = crypto.createHash("sha512");
h2.update("ab");
const d = h2.digest();
console.log("buf:", typeof d, /^[0-9a-f]{128}$/.test(d.toString("hex")));
// base64
console.log("b64:", crypto.createHash("md5").update("hello world").digest("base64"));
// repeat digest + update after digest
const h3 = crypto.createHash("md5");
h3.update("ab");
const f1 = h3.digest("hex");
const f2 = h3.digest("hex");
h3.update("cd");
console.log("idem:", f1 === f2, f1, h3.digest("hex"));
// algorithm property
const h4 = crypto.createHash("sha384");
console.log("algo:", h4.algorithm);
h4.update("x");
console.log("algo2:", h4.digest("hex"));
// binary data via Buffer
console.log("bin:", crypto.createHash("sha256").update(Buffer.from([0, 1, 2, 250, 255])).digest("hex"));
"#,
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    let expected = r#"md5 5eb63bbbe01eeed093cb22bb8f5acdc3
sha1 2aae6c35c94fcfb415dbe95f408b9ce91ee846ed
sha256 b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9
sha384 fdbd8e75a67f29f701a4e040385e2e23986303ea10239211af907fcbb83578b3e417cb71ce646efd0819dd8c088de1bd
sha512 309ecc489c12d6eb4cc40f50c902f2b4d0ed77ee511a7c7a9bcd3ca86d4cd86f989dd35bc5ff499670da34255b45b0cfd830e81f605dcf7dc5542e93ae9cd76f
chain: a8294749cd255b342a5420b5dc6fb4d6122e62bbae0497554190ec0eac44176c
buf: object true
b64: XrY7u+Ae7tCTyyK7j1rNww==
idem: true 187ef4436122d1cc2f40dc2b92f0eba0 e2fc714c4727ee9395f324cd2e7f331f
algo: sha384
algo2: d752c2c51fba0e29aa190570a9d4253e44077a058d3297fa3a5630d5bd012622f97c28acaed313b5c83bb990caa7da85
bin: ad6170c304e1b0a6cfe3b1ba6b40c065f646428f623f664ad31d54b98735195c"#;
    assert_eq!(out, expected);
}

/// hash 一次性哈希与 getHashes/getCiphers 确定性数组、模块表面 typeof 对拍
#[test]
fn hash_oneshot_gethashes_ciphers_e2e_matches_go() {
    let work = work_dir("p02_oneshot");
    std::fs::write(
        work.join("probe.js"),
        r#"const Buffer = require("buffer").Buffer;
const crypto = require("crypto");
console.log(crypto.hash("sha256", "abc"));
console.log(crypto.hash("md5", "hello", "hex"));
console.log(crypto.hash("sha1", "hello", "base64"));
console.log(typeof crypto.hash("sha256", "abc", "buffer"));
console.log(crypto.hash("sha512", Buffer.from("payload")));
console.log(crypto.getHashes().join(","));
console.log(crypto.getCiphers().join(","));
console.log(typeof crypto.createHash, typeof crypto.createHmac, typeof crypto.createCipheriv, typeof crypto.createDecipheriv);
console.log(typeof crypto.pbkdf2Sync, typeof crypto.pbkdf2, typeof crypto.scryptSync, typeof crypto.scrypt);
console.log(typeof crypto.hkdfSync, typeof crypto.hkdf, typeof crypto.randomBytes, typeof crypto.randomUUID);
console.log(typeof crypto.randomInt, typeof crypto.randomFillSync, typeof crypto.randomFill, typeof crypto.timingSafeEqual);
console.log(typeof crypto.createSecretKey, typeof crypto.checkPrimeSync, typeof crypto.checkPrime, typeof crypto.X509Certificate);
console.log(typeof crypto.createPrivateKey, typeof crypto.getRandomValues, typeof crypto.webcrypto);
"#,
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    let expected = r#"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
5d41402abc4b2a76b9719d911017c592
qvTGHdzF6KLavt4PO0gs2a6pQ00=
object
70b33ce9c9047e30f917e7ea13e42f7767008c3f4f9c9baf49e4390fc625549e9625eee39b94545074e8a1824cf3f238463b11bc03d97348e0fc2999ca1fff7f
md5,sha1,sha256,sha384,sha512
aes-128-cbc,aes-192-cbc,aes-256-cbc,aes-128-ecb,aes-256-ecb,aes-128-ctr,aes-256-ctr,aes-128-gcm,aes-256-gcm
function function function function
function function function function
function function function function
function function function function
function function function function
function function object"#;
    assert_eq!(out, expected);
}

/// createHmac 全算法族与链式 update/digest 对拍
#[test]
fn hmac_family_e2e_matches_go() {
    let work = work_dir("p03_hmac");
    std::fs::write(
        work.join("probe.js"),
        r#"const Buffer = require("buffer").Buffer;
const crypto = require("crypto");
const cases = [
  ["sha256", "key", "The quick brown fox jumps over the lazy dog"],
  ["sha1", "secret", "some data to sign"],
  ["md5", "secret", "datamore"],
  ["sha384", "key", "The quick brown fox jumps over the lazy dog"],
  ["sha512", "key", "The quick brown fox jumps over the lazy dog"],
];
for (let i = 0; i < cases.length; i++) {
  const c = cases[i];
  console.log(c[0], crypto.createHmac(c[0], c[1]).update(c[2]).digest("hex"));
}
// chaining + base64 + algorithm + buffer key
const hm = crypto.createHmac("sha256", "twenty-byte-key!!");
hm.update("Hi There");
console.log("hmac-buf:", hm.digest("hex"));
console.log("hmac-b64:", crypto.createHmac("sha1", "secret").update("data").update("more").digest("base64"));
const hm2 = crypto.createHmac("sha512", "k");
console.log("hmac-algo:", hm2.algorithm);
hm2.update("x");
console.log("hmac-digest-len:", hm2.digest("buffer").length);
"#,
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    let expected = r#"sha256 f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8
sha1 5016a12969b8b7f43a4080d8d389e72eb1b2958b
md5 97f0a605bf0cde4189662445373eb4cb
sha384 d7f4727e2c0b39ae0f1e40cc96f60242d5b7801841cea6fc592c5d3e1ae50700582a96cf35e1e554995fe4e03381c237
sha512 b42af09057bac1e2d41708e48a902e09b5ff7f12ab428a4fe86653c73dd248fb82f948a549f7b791a5b41915ee4d1ec3935357e4e2317250d0372afa2ebeeb3a
hmac-buf: 91a452b72fd46c3150e5342d3649747227b7c8947506857f6c0931c5dd614807
hmac-b64: MMuPV0n+M+CQ5c4lw19e4DSkr+A=
hmac-algo: sha512
hmac-digest-len: 64"#;
    assert_eq!(out, expected);
}

/// createCipheriv/createDecipheriv aes-128/256-cbc 加解密往返与已知向量对拍
#[test]
fn cipher_cbc_roundtrip_e2e_matches_go() {
    let work = work_dir("p04_cbc");
    std::fs::write(
        work.join("probe.js"),
        r#"const Buffer = require("buffer").Buffer;
const crypto = require("crypto");
const key128 = Buffer.from("0123456789abcdef");
const iv = Buffer.from("0123456789abcdef");
const ci = crypto.createCipheriv("aes-128-cbc", key128, iv);
const upd = ci.update("Hello, World!");
const enc = ci.final();
console.log("upd-empty:", upd.length, "enc128:", enc.toString("hex"), enc.length);
const de = crypto.createDecipheriv("aes-128-cbc", key128, iv);
de.update(enc);
console.log("dec128:", de.final().toString());
// aes-256-cbc multi-update
const key256 = "0123456789abcdef0123456789abcdef";
const iv2 = "fedcba9876543210";
const ci2 = crypto.createCipheriv("aes-256-cbc", key256, iv2);
ci2.update("Hello, ");
ci2.update("");
ci2.update("World!");
const enc2 = ci2.final();
console.log("enc256:", enc2.toString("hex"));
const de2 = crypto.createDecipheriv("aes-256-cbc", key256, iv2);
de2.update(enc2);
console.log("dec256:", de2.final().toString());
// empty plaintext
const ci3 = crypto.createCipheriv("aes-128-cbc", key128, iv);
ci3.update("");
console.log("empty:", ci3.final().toString("hex"));
// string key/iv
const ci4 = crypto.createCipheriv("aes-128-cbc", "0123456789abcdef", "0123456789abcdef");
ci4.update("string key");
console.log("strkey:", ci4.final().toString("hex"));
// final twice + update after final
const ci5 = crypto.createCipheriv("aes-128-cbc", key128, iv);
ci5.update("z");
const f1 = ci5.final().toString("hex");
const f2 = ci5.final().toString("hex");
console.log("final2:", f1 === f2, f1);
// binary ciphertext roundtrip
const bin = Buffer.from([0x00, 0xff, 0x10, 0x20, 0x30]);
const ci6 = crypto.createCipheriv("aes-128-cbc", key128, iv);
ci6.update(bin);
const enc6 = ci6.final();
const de6 = crypto.createDecipheriv("aes-128-cbc", key128, iv);
de6.update(enc6);
const back = de6.final();
console.log("bin-rt:", back.length, bin.equals(back));
"#,
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    let expected = r#"upd-empty: 0 enc128: 0b3a07351d769dc5405b3b058e6a1720 16
dec128: Hello, World!
enc256: 41b0fbbcf5aa249b420a6d0c817f0f5d
dec256: Hello, World!
empty: ed47fee0545c3fa7dd070d44b86e98d9
strkey: 847ab11c941ef6a26befb2f234a8824c
final2: true 561ad671efbdf93200bc5a1d20a4b72c
bin-rt: 5 true"#;
    assert_eq!(out, expected);
}

/// ecb/ctr/gcm（getAuthTag/setAuthTag）与 aes-192-cbc 对拍
#[test]
fn cipher_modes_ecb_ctr_gcm_e2e_matches_go() {
    let work = work_dir("p05_modes");
    std::fs::write(
        work.join("probe.js"),
        r#"const Buffer = require("buffer").Buffer;
const crypto = require("crypto");
// ecb (iv null)
const ec = crypto.createCipheriv("aes-128-ecb", "0123456789abcdef", null);
ec.update("ecb mode!!");
const ecbEnc = ec.final();
console.log("ecb:", ecbEnc.toString("hex"));
const ed = crypto.createDecipheriv("aes-128-ecb", "0123456789abcdef", null);
ed.update(ecbEnc);
console.log("ecb-dec:", ed.final().toString());
// ctr (arbitrary length)
const ct = crypto.createCipheriv("aes-128-ctr", Buffer.from("0123456789abcdef"), "fedcba9876543210");
ct.update("ctr mode data");
const ctrEnc = ct.final();
console.log("ctr:", ctrEnc.toString("hex"), ctrEnc.length);
const cd = crypto.createDecipheriv("aes-128-ctr", Buffer.from("0123456789abcdef"), "fedcba9876543210");
cd.update(ctrEnc);
console.log("ctr-dec:", cd.final().toString());
// gcm roundtrip with auth tag
const gk = Buffer.from("0123456789abcdef0123456789abcdef");
const gn = "123456789abc";
const gc = crypto.createCipheriv("aes-256-gcm", gk, gn);
gc.update("hello gcm");
const gcmEnc = gc.final();
const gtag = gc.getAuthTag();
console.log("gcm:", gcmEnc.toString("hex"), "tag:", gtag.toString("hex"));
const gd = crypto.createDecipheriv("aes-256-gcm", gk, gn);
gd.setAuthTag(gtag);
gd.update(gcmEnc);
console.log("gcm-dec:", gd.final().toString());
// 192-cbc
const c192 = crypto.createCipheriv("aes-192-cbc", Buffer.from("0123456789abcdef01234567"), "fedcba9876543210");
c192.update("192");
console.log("192:", c192.final().toString("hex"));
"#,
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    let expected = r#"ecb: e667a29c452fb4230fdec753a1167e22
ecb-dec: ecb mode!!
ctr: 683fa4519b1f1b6c985ca2a2ab 13
ctr-dec: ctr mode data
gcm: c92d3969211b50acb5 tag: cc654c9ece8f0df34faada4029ab728a
gcm-dec: hello gcm
192: 1096f6a7c80b50d241c3da83fdba3ea2"#;
    assert_eq!(out, expected);
}

/// pbkdf2Sync/scryptSync/hkdfSync/createSecretKey 对拍
#[test]
fn kdf_sync_e2e_matches_go() {
    let work = work_dir("p06_kdf");
    std::fs::write(
        work.join("probe.js"),
        r#"const Buffer = require("buffer").Buffer;
const crypto = require("crypto");
const k = crypto.pbkdf2Sync("password", "salt", 1, 20, "sha1");
console.log("pb1:", k.toString("hex"), typeof k, k.length);
const k2 = crypto.pbkdf2Sync(Buffer.from("password"), Buffer.from("salt"), 2, 25, "sha256");
console.log("pb2:", k2.toString("hex"));
const k3 = crypto.pbkdf2Sync("passwordPASSWORDpassword", "saltSALTsaltSALTsaltSALTsaltSALTsalt", 4096, 25, "sha1");
console.log("pb3:", k3.toString("hex"));
const k4 = crypto.pbkdf2Sync("password", "salt", 1, 64, "sha512");
console.log("pb4:", /^[0-9a-f]{128}$/.test(k4.toString("hex")), k4.length);
// default digest sha1
const k5 = crypto.pbkdf2Sync("password", "salt", 1, 20);
console.log("pb5:", k5.toString("hex") === k.toString("hex"));
// scryptSync small params
const s = crypto.scryptSync("password", "salt", 10, { N: 16, r: 1, p: 1 });
console.log("sc:", typeof s, s.toString("hex"));
const s2 = crypto.scryptSync("password", "salt", 32, { N: 16, r: 1, p: 1 });
console.log("sc2:", s2.toString("hex"), s2.length);
// hkdfSync
const hk = crypto.hkdfSync("sha256", "secret", "salt", "info", 16);
console.log("hkdf:", hk.toString("hex"), hk.length);
const hk2 = crypto.hkdfSync("sha1", Buffer.from("0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b"), Buffer.alloc(0), Buffer.alloc(0), 42);
console.log("hkdf2:", hk2.toString("hex"));
// createSecretKey
const sk = crypto.createSecretKey("topsecret");
console.log("sk:", sk.type, sk.symmetricKeySize, sk.export().toString("hex"), typeof sk.export);
const sk2 = crypto.createSecretKey(Buffer.from([1, 2, 3, 4, 5, 6, 7, 8]));
console.log("sk2:", sk2.type, sk2.symmetricKeySize, sk2.export().toString("hex"));
"#,
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    let expected = r#"pb1: 0c60c80f961f0e71f3a9b524af6012062fe037a6 object 20
pb2: ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6
pb3: 3d2eec4fe41c849b80c8d83662c0e44a8b291a964cf2f07038
pb4: true 64
pb5: true
sc: object 45133c3dfba48c82235d
sc2: 45133c3dfba48c82235df51a5349924110eee893752f0d4168d2e2aee5722d82 32
hkdf: f6d2fcc47cb939deafe3853a1e641a27 16
hkdf2: 16aeb99c7ae33f7e79109816e470e656758bd45189412a81edb8b1c230e9c10f811b4cead9bec18e86fd
sk: secret 9 746f70736563726574 function
sk2: secret 8 0102030405060708"#;
    assert_eq!(out, expected);
}

/// pbkdf2 异步回调（微任务先于宏任务投递顺序）对拍
#[test]
fn pbkdf2_async_callback_e2e_matches_go() {
    let work = work_dir("p07_pbkdf2_async");
    std::fs::write(
        work.join("probe.js"),
        r#"const Buffer = require("buffer").Buffer;
const crypto = require("crypto");
console.log("before");
crypto.pbkdf2("password", "salt", 1, 20, "sha1", (err, key) => {
  console.log("cb:", err === null, key.toString("hex"), typeof key);
});
Promise.resolve().then(() => console.log("tick"));
console.log("after");
"#,
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    let expected = r#"before
after
tick
cb: true 0c60c80f961f0e71f3a9b524af6012062fe037a6 object"#;
    assert_eq!(out, expected);
}

/// scrypt 异步回调对拍
#[test]
fn scrypt_async_callback_e2e_matches_go() {
    let work = work_dir("p08_scrypt_async");
    std::fs::write(
        work.join("probe.js"),
        r#"const Buffer = require("buffer").Buffer;
const crypto = require("crypto");
console.log("sync-first:", crypto.scryptSync("password", "salt", 8, { N: 16, r: 1, p: 1 }).toString("hex"));
crypto.scrypt("password", "salt", 8, { N: 16, r: 1, p: 1 }, (err, key) => {
  console.log("cb:", err === null, key.toString("hex"));
});
console.log("after");
"#,
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    let expected = r#"sync-first: 45133c3dfba48c82
after
cb: true 45133c3dfba48c82"#;
    assert_eq!(out, expected);
}

/// hkdf 异步回调对拍
#[test]
fn hkdf_async_callback_e2e_matches_go() {
    let work = work_dir("p09_hkdf_async");
    std::fs::write(
        work.join("probe.js"),
        r#"const Buffer = require("buffer").Buffer;
const crypto = require("crypto");
crypto.hkdf("sha256", "secret", "salt", "info", 16, (err, out) => {
  console.log("hkdf-cb:", err === null, out.toString("hex"));
});
console.log("after");
"#,
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    let expected = r#"after
hkdf-cb: true f6d2fcc47cb939deafe3853a1e641a27"#;
    assert_eq!(out, expected);
}

/// randomBytes/randomUUID/randomInt/randomFillSync/randomFill 长度格式与取值域对拍
#[test]
fn random_surface_e2e_matches_go() {
    let work = work_dir("p10_random");
    std::fs::write(
        work.join("probe.js"),
        r#"const Buffer = require("buffer").Buffer;
const crypto = require("crypto");
const rb = crypto.randomBytes(16);
console.log("rb:", rb.length, typeof rb, /^[0-9a-f]{32}$/.test(rb.toString("hex")));
const small = crypto.randomBytes(1);
console.log("rb1:", small.length, typeof small[0]);
const u = crypto.randomUUID();

console.log("uuid:", typeof u, /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(u));
console.log("ri-type:", typeof crypto.randomInt(10), typeof crypto.randomInt(5, 10));
let ok = true;
for (let i = 0; i < 30; i++) { const v = crypto.randomInt(3, 7); if (v < 3 || v >= 7) ok = false; }
console.log("ri-range:", ok);
const bf = Buffer.alloc(8);
const ret = crypto.randomFillSync(bf);
console.log("rfs:", ret === bf, bf.length);
crypto.randomFill(bf, (err, buf) => console.log("fill-cb:", err === null, buf === bf, buf.length));
"#,
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    let expected = r#"rb: 16 object true
rb1: 1 number
uuid: string true
ri-type: number number
ri-range: true
rfs: true 8
fill-cb: true true 8"#;
    assert_eq!(out, expected);
}

/// timingSafeEqual 真值与错误路径对拍
#[test]
fn timing_safe_equal_e2e_matches_go() {
    let work = work_dir("p11_tse");
    std::fs::write(
        work.join("probe.js"),
        r#"const Buffer = require("buffer").Buffer;
const crypto = require("crypto");
const a = Buffer.from("abcd"), b = Buffer.from("abcd"), e = Buffer.from("abce");
console.log("eq:", crypto.timingSafeEqual(a, b), crypto.timingSafeEqual(a, e));
const long1 = crypto.randomBytes(64);
const long2 = crypto.randomBytes(64);
console.log("self:", crypto.timingSafeEqual(long1, long1), typeof crypto.timingSafeEqual(a, b));
try { crypto.timingSafeEqual(Buffer.from("a"), Buffer.from("ab")); } catch (err) { console.log("len-err:", err.message); }
try { crypto.timingSafeEqual("str", "str"); } catch (err) { console.log("type-err:", err.message); }
try { crypto.timingSafeEqual(Buffer.from("a")); } catch (err) { console.log("arg-err:", err.message); }
"#,
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    let expected = r#"eq: true false
self: true boolean
len-err: timingSafeEqual: input buffers must have the same length
type-err: timingSafeEqual: arguments must be Buffer, TypedArray, or DataView
arg-err: timingSafeEqual: two buffers required"#;
    assert_eq!(out, expected);
}

/// 全部错误路径消息逐字对拍
#[test]
fn crypto_error_paths_e2e_matches_go() {
    let work = work_dir("p12_errors");
    std::fs::write(
        work.join("probe.js"),
        r#"const Buffer = require("buffer").Buffer;
const crypto = require("crypto");
function t(name, fn) { try { const r = fn(); console.log(name, "ok"); } catch (e) { console.log(name, "threw:", e.message); } }
const k256 = Buffer.from("0123456789abcdef0123456789abcdef");
const n12 = "123456789abc";
t("gcm-notag", () => { const d = crypto.createDecipheriv("aes-256-gcm", k256, n12); d.update(Buffer.from("00", "hex")); return d.final(); });
t("gettag-early", () => crypto.createCipheriv("aes-256-gcm", k256, n12).getAuthTag());
t("gcm-badtag", () => {
  const c = crypto.createCipheriv("aes-256-gcm", k256, n12);
  c.update("data");
  const enc = c.final();
  const other = crypto.createCipheriv("aes-256-gcm", k256, "123456789abc");
  other.update("data");
  other.final();
  const wrongTag = other.getAuthTag();
  const d = crypto.createDecipheriv("aes-256-gcm", k256, n12);
  d.setAuthTag(wrongTag);
  d.update(enc);
  return d.final();
});
t("sk-noarg", () => crypto.createSecretKey());
t("hkdf-short", () => crypto.hkdfSync("sha256", "ikm", "salt", "info", 4));
t("hkdf-baddigest", () => crypto.hkdfSync("md6", "ikm", "salt", "info", 4));
"#,
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    let expected = r#"gcm-notag threw: cipher: setAuthTag must be called before final for AES-GCM
gettag-early threw: getAuthTag: no auth tag available (call final first)
gcm-badtag ok
sk-noarg threw: createSecretKey: key argument required
hkdf-short ok
hkdf-baddigest threw: createHash: unsupported algorithm "md6""#;
    assert_eq!(out, expected);
}

/// checkPrimeSync/checkPrime 异步回调对拍
#[test]
fn checkprime_e2e_matches_go() {
    let work = work_dir("p13_prime_fill");
    std::fs::write(
        work.join("probe.js"),
        r#"const Buffer = require("buffer").Buffer;
const crypto = require("crypto");
console.log("prime:", crypto.checkPrimeSync(7), crypto.checkPrimeSync(8), crypto.checkPrimeSync(2), crypto.checkPrimeSync(1));
console.log("prime-big:", crypto.checkPrimeSync(104729), crypto.checkPrimeSync(104723));

crypto.checkPrime(11, (err, r) => console.log("prime-cb:", err === null, r));
const bf = Buffer.alloc(8);
crypto.randomFillSync(bf);
console.log("rfs:", bf.length);
console.log("after");
"#,
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    let expected = r#"prime: true false true false
prime-big: true true
rfs: 8
after
prime-cb: true true"#;
    assert_eq!(out, expected);
}

/// X509Certificate/createPrivateKey 证书解析、校验方法与错误路径对拍
#[test]
fn x509_certificate_e2e_matches_go() {
    let work = work_dir("p14_x509");
    std::fs::write(
        work.join("probe.js"),
        r#"const crypto = require("crypto");
const caPem = `-----BEGIN CERTIFICATE-----
MIICUDCCAbmgAwIBAgIUUJWjRVte8z2FM4quheYxqQBpXU8wDQYJKoZIhvcNAQEL
BQAwOjELMAkGA1UEBhMCVVMxEzARBgNVBAoMCkFsdWthIFJvb3QxFjAUBgNVBAMM
DUFsdWthIFJvb3QgQ0EwHhcNMjYwOTA0MTc0NzU4WhcNMzYwOTAxMTc0NzU4WjA6
MQswCQYDVQQGEwJVUzETMBEGA1UECgwKQWx1a2EgUm9vdDEWMBQGA1UEAwwNQWx1
a2EgUm9vdCBDQTCBnzANBgkqhkiG9w0BAQEFAAOBjQAwgYkCgYEA1VZxwM28KYhe
jWv2Ffc6nVhMzEOmvOAFZkJL1+ndFdnn09nqk81r5kNavS8Bh+nWMA+NpDVuGV/g
amZ9vLl5ULa7HBCc5mj8xieEYYc7/duqENvxKwswv0z1VvQtAgedFl6TUuFKKct4
OHalwUvEOyBlOejam+UdYqUZ5tkN/90CAwEAAaNTMFEwHQYDVR0OBBYEFIpvBnvh
YH0NoE7CDP+Vk5dw18J8MB8GA1UdIwQYMBaAFIpvBnvhYH0NoE7CDP+Vk5dw18J8
MA8GA1UdEwEB/wQFMAMBAf8wDQYJKoZIhvcNAQELBQADgYEAZz3ILvHUrv3ahHex
J+sFng6kb59Biui+MlD5bJozz17ne0DhUlV45pu1/IRGPJ/W171JaBQLYV+1wBIn
1leiK53ppVY84NI0vaBEKn5sgjkBVWDvuBfJa1iHlTDytVd55+GHcY9ylBU6dobH
5ntutcSEUCbH3n1GTeUipPwpNv0=
-----END CERTIFICATE-----`;
const leafPem = `-----BEGIN CERTIFICATE-----
MIICPTCCAaagAwIBAgIUA7KHDeqS5y/HagH2mryP8szlgDwwDQYJKoZIhvcNAQEL
BQAwOjELMAkGA1UEBhMCVVMxEzARBgNVBAoMCkFsdWthIFJvb3QxFjAUBgNVBAMM
DUFsdWthIFJvb3QgQ0EwHhcNMjYwOTA0MTc0NzU5WhcNMzYwOTAxMTc0NzU5WjA4
MQswCQYDVQQGEwJVUzEOMAwGA1UECgwFQWx1a2ExGTAXBgNVBAMMEGxlYWYuZXhh
bXBsZS5jb20wgZ8wDQYJKoZIhvcNAQEBBQADgY0AMIGJAoGBAKEddPWZuoGeWmHr
r+uI2VKB5tGR4+jc5TMMxm/MCdsO7zBdg4SbBfhwLEZ0lHKr8z6wbbxLfSLmSq6e
sI4oqwcjiMu2796XRiG0PJ/pb4tP/w7H4HMk0INwKvkfDKhOTJhJfJ94dG8K7SnG
vjFgIIzpH7PL3MkreYEmoEXDF1RXAgMBAAGjQjBAMB0GA1UdDgQWBBSn6d13hlQF
kyWmr2EKK2Go6IaP9zAfBgNVHSMEGDAWgBSKbwZ74WB9DaBOwgz/lZOXcNfCfDAN
BgkqhkiG9w0BAQsFAAOBgQApx7ZGTfsL0ce3aEuuszeMSSb7TKfI64+5QtK4/vJy
cuH3MEhoEjfJrX9QjI5RRCQvRDjReYrhgdGetFse3GBGwyPfCtwsiF5pHYeTh8k7
nOHMLuGizezjBC5dE13BfvOMhxMH29TAs5jpOqbC4Gs449vRke3b8yeY2Tt7E8zM
5w==
-----END CERTIFICATE-----`;
const leafKeyPem = `-----BEGIN PRIVATE KEY-----
MIICeAIBADANBgkqhkiG9w0BAQEFAASCAmIwggJeAgEAAoGBAKEddPWZuoGeWmHr
r+uI2VKB5tGR4+jc5TMMxm/MCdsO7zBdg4SbBfhwLEZ0lHKr8z6wbbxLfSLmSq6e
sI4oqwcjiMu2796XRiG0PJ/pb4tP/w7H4HMk0INwKvkfDKhOTJhJfJ94dG8K7SnG
vjFgIIzpH7PL3MkreYEmoEXDF1RXAgMBAAECgYAxhs6XWQReKAF8rGjNrKmxlUER
FxnKUW0bfkfZwg0di7+3TGfLcaQqNMFHfzrK7VS+5pk1EreK7OP0Pc/kQ1gfQs5p
du37wAxSxR6wSv68sk4t1Cch15joDcgeuFMEd21Y/Tr1xTD26IqVQ/jQ+o6xWYk+
NUpanpic1pvyaPIpIQJBAMv0FjGLhyNBvLWdJEes2pP6fIfQ9tUtap1F5iEteuyT
TnwSXmukHGKhEJymYRjeTSEMI+EP8sG6/zAsktefUw8CQQDKOtCLhhvbGOtEMHRe
CxMgRE8gWz8HCzl5AGnt0Cw0/yW7XxWdYyqcSF1lLj4Tq+x4UXmfNdnvAIQogiwV
rMo5AkEAhtNpCH+wakI+ueCT5z4BkOl6AV7Gjc5kOGvI4g3qwRHwRFzwRkBK83h+
PtBOR95NJpeb8GBWnnM712DgAeK1SQJBAKN+mUuzyKGBq/MdGXdOjM/xaedG3dXc
BUMGSp2xR4wxG1g4r0jm+3QOLTO4BwfwXuWHOUS2TNMlH7OAShPb9kECQQCy7ELz
NPIP6kwqXrdBn22IScB69+8vQ725M1308GTyo++9K46/a3DFfQemYL5dzTXfg1Fd
3fNgtiyMcb55pHtC
-----END PRIVATE KEY-----`;
const ca = new crypto.X509Certificate(caPem);
const leaf = new crypto.X509Certificate(leafPem);
const key = crypto.createPrivateKey(leafKeyPem);
console.log("ca-subject:", ca.subject === "C=US\nO=Aluka Root\nCN=Aluka Root CA");
console.log("ca-issuer:", ca.issuer === "C=US\nO=Aluka Root\nCN=Aluka Root CA");
console.log("ca-validFrom:", ca.validFrom);
console.log("ca-validTo:", ca.validTo);
console.log("ca-serial:", ca.serialNumber);
console.log("ca-fp:", ca.fingerprint);
console.log("ca-fp256:", ca.fingerprint256);
console.log("ca-fp512:", ca.fingerprint512);
console.log("ca-ca:", ca.ca, ca.raw.length, typeof ca.subjectAltName);
console.log("leaf-subject:", leaf.subject === "C=US\nO=Aluka\nCN=leaf.example.com");
console.log("leaf-issuer:", leaf.issuer === "C=US\nO=Aluka Root\nCN=Aluka Root CA");
console.log("leaf-ca:", leaf.ca, typeof leaf.subjectAltName);
console.log("leaf-fp:", leaf.fingerprint);
console.log("leaf-fp256:", leaf.fingerprint256);
console.log("issued:", leaf.checkIssued(ca), ca.checkIssued(ca), ca.checkIssued(leaf));
console.log("key:", key.type, key.asymmetricKeyType);
console.log("checkPK:", leaf.checkPrivateKey(key), "caPK:", ca.checkPrivateKey(key));
console.log("verify-key:", leaf.verify(key), "verify-pub:", leaf.verify(leaf.publicKey));
console.log("pub:", leaf.publicKey.type, leaf.publicKey.asymmetricKeyType, leaf.publicKey.raw.length);
console.log("host1:", leaf.checkHost("leaf.example.com"), "host2:", ca.checkHost("aluka.root"));
const pem = leaf.toString();
console.log("pem-ok:", /^-----BEGIN CERTIFICATE-----/.test(pem), typeof pem);
const lo = leaf.toLegacyObject();
console.log("lo-subject:", lo.subject.C, lo.subject.O, lo.subject.CN);
console.log("lo-ca:", lo.ca, "lo-bits:", lo.bits, "lo-exp:", lo.exponent);
console.log("lo-mod:", /^[0-9A-F]{256}$/.test(lo.modulus), typeof lo.modulus);
console.log("lo-vf:", lo.valid_from, "|", lo.valid_to);
console.log("lo-pub:", lo.pubkey.length, "lo-raw-same:", lo.raw.length === leaf.raw.length);
console.log("from-der:", new crypto.X509Certificate(leaf.raw).serialNumber === leaf.serialNumber);
try { new crypto.X509Certificate("not a cert"); } catch (e) { console.log("bad-pem:", e.message); }
try { crypto.createPrivateKey("junk"); } catch (e) { console.log("bad-key:", e.message); }
try { new crypto.X509Certificate(); } catch (e) { console.log("no-arg:", e.message); }
"#,
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    let expected = r#"ca-subject: true
ca-issuer: true
ca-validFrom: Sep  4 17:47:58 2026 GMT
ca-validTo: Sep  1 17:47:58 2036 GMT
ca-serial: 5095A3455B5EF33D85338AAE85E631A900695D4F
ca-fp: 1D:F1:8F:44:2E:34:38:B0:51:0A:E0:2D:47:CB:80:9E:45:79:AC:D2
ca-fp256: B7:35:5E:20:6B:C7:11:19:85:9A:90:0C:A4:52:C1:C8:86:D8:93:7B:EA:12:14:AF:A2:72:55:23:71:A3:18:1E
ca-fp512: E9:04:1A:B5:74:45:E1:BF:37:DC:86:B0:18:87:BC:E4:7F:EB:29:11:03:4D:1E:49:58:49:F0:91:1B:C9:C3:DC:B6:4A:5A:F1:93:4B:D8:DB:E8:AA:51:2D:2A:E8:A4:24:75:CD:1C:B6:B6:43:58:E2:C9:35:A5:3F:E5:5F:7C:55
ca-ca: true 596 undefined
leaf-subject: true
leaf-issuer: true
leaf-ca: false undefined
leaf-fp: C1:1C:AE:1C:7D:9D:0A:2F:3C:97:6E:0D:B3:7F:F6:22:80:86:EA:EF
leaf-fp256: D5:D9:4D:95:32:F8:85:D0:34:D9:DB:25:A3:2C:BE:06:F5:8B:1A:91:EA:CB:AD:D9:BD:26:8C:91:56:EC:48:04
issued: true true false
key: private rsa
checkPK: true caPK: false
verify-key: true verify-pub: false
pub: public rsa 162
host1: undefined host2: undefined
pem-ok: true string
lo-subject: US Aluka leaf.example.com
lo-ca: false lo-bits: 1024 lo-exp: 0x10001
lo-mod: true string
lo-vf: Sep  4 17:47:59 2026 UTC | Sep  1 17:47:59 2036 UTC
lo-pub: 162 lo-raw-same: true
from-der: true
bad-pem: X509Certificate: invalid PEM certificate
bad-key: createPrivateKey: invalid PEM
no-arg: X509Certificate: certificate argument required"#;
    assert_eq!(out, expected);
}

/// randomInt 异步回调与 getRandomValues 原地填充对拍
#[test]
fn randomint_async_getrandomvalues_e2e_matches_go() {
    let work = work_dir("p15_ri_async");
    std::fs::write(
        work.join("probe.js"),
        r#"const Buffer = require("buffer").Buffer;
const crypto = require("crypto");
const gbuf = Buffer.alloc(16);
crypto.getRandomValues(gbuf);
console.log("grv:", gbuf.length, typeof crypto.getRandomValues);
let inRange = null;
crypto.randomInt(10, 20, (err, v) => {
  inRange = v >= 10 && v < 20;
  console.log("ri-cb:", err === null, inRange);
});
console.log("after");
"#,
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    let expected = r#"grv: 16 function
after
ri-cb: true true"#;
    assert_eq!(out, expected);
}
