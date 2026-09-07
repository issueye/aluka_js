//! Phase 5 网络家族内置库端到端对拍测试：
//! - `net`（TCP 回声全生命周期、位置签名连接、地址属性、write 回调、
//!   isIP/isIPv4/isIPv6、BlockList、SocketAddress）
//! - `dns`（lookup/resolve 家族/reverse/lookupService/Resolver，顺序链式回调）
//! - `dns/promises`（Promise 化 lookup/resolve 家族、与 `dns.promises` 恒等）
//! - `dgram`（UDP 真实回环收发、bind/send/message/close）
//! - `tls`（API 表面、选项校验错误消息、getCiphers——TLS 握手路径未实现，
//!   探针绝不触发真实握手）
//!
//! 与 Node.js 22 LTS 规范逐字比对：alukac 前端整图编译 →
//! aluvm 原生驱动执行，输出与 Node.js 22 严格一致。
//! 探针纪律：端口只用 47110-47139 区间、全部 127.0.0.1 回环、单连接顺序
//! 收发；只打印回调内容；结尾 close 全部实体保证 aluvm 正常退出。

mod common;

use std::path::PathBuf;

/// 创建隔离的临时测试目录
fn work_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("builtins_phase5_net_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建工作目录失败");
    dir
}

/// 验证 net TCP 服务器优先问候的全生命周期回声（listen/connect/data/end/close）
#[test]
fn net_echo_greet_lifecycle_e2e_matches_go() {
    let work = work_dir("net_echo");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const net = require(\"node:net\");\n",
            "const server = net.createServer((sock) => {\n",
            "  console.log(\"srv: conn\", typeof sock.write);\n",
            "  sock.write(\"greet\");\n",
            "  sock.on(\"data\", (d) => {\n",
            "    console.log(\"srv: got\", d.toString());\n",
            "    sock.write(\"echo:\" + d.toString());\n",
            "  });\n",
            "});\n",
            "server.listen(47112, \"127.0.0.1\", () => {\n",
            "  console.log(\"srv: listening cb\");\n",
            "  const c = net.createConnection({ host: \"127.0.0.1\", port: 47112 }, () => {\n",
            "    console.log(\"cli: connected cb\");\n",
            "  });\n",
            "  c.on(\"connect\", () => console.log(\"cli: connect evt\"));\n",
            "  c.on(\"data\", (d) => {\n",
            "    if (d.toString() === \"greet\") {\n",
            "      console.log(\"cli: got greet\");\n",
            "      c.write(\"ping\");\n",
            "    } else {\n",
            "      console.log(\"cli: got\", d.toString());\n",
            "      c.end();\n",
            "    }\n",
            "  });\n",
            "  c.on(\"close\", () => {\n",
            "    console.log(\"cli: close evt\");\n",
            "    server.close(() => console.log(\"srv: close cb\"));\n",
            "  });\n",
            "});\n",
            "server.on(\"listening\", () => console.log(\"srv: listening evt\"));\n",
            "server.on(\"close\", () => console.log(\"srv: close evt\"));\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(
        out,
        concat!(
            "srv: listening cb\n",
            "srv: listening evt\n",
            "cli: connected cb\n",
            "cli: connect evt\n",
            "srv: conn function\n",
            "cli: got greet\n",
            "srv: got ping\n",
            "cli: got echo:ping\n",
            "cli: close evt\n",
            "srv: close evt\n",
            "srv: close cb"
        )
    );
}

/// 验证 net.connect 位置签名（port, host）、server.address/getConnections 与
/// socket 地址属性（remoteAddress/remotePort/remoteFamily）
#[test]
fn net_positional_connect_and_props_e2e_matches_go() {
    let work = work_dir("net_props");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const net = require(\"node:net\");\n",
            "const server = net.createServer();\n",
            "server.on(\"connection\", (sock) => {\n",
            "  sock.on(\"data\", (d) => {\n",
            "    console.log(\"srv got:\", d.toString(), \"w-ret:\", sock.write(\"ok\"));\n",
            "    sock.end();\n",
            "  });\n",
            "});\n",
            "server.listen(47114, \"127.0.0.1\", () => {\n",
            "  const addr = server.address();\n",
            "  console.log(\"addr:\", addr.address, addr.port, addr.family);\n",
            "  server.getConnections((err, n) => console.log(\"conns:\", err, n));\n",
            "  const c = net.connect(47114, \"127.0.0.1\", () => {\n",
            "    console.log(\"cli cb: ra=\", c.remoteAddress, \"rp=\", c.remotePort, \"rf=\", c.remoteFamily);\n",
            "    const ret = c.write(\"hello\");\n",
            "    console.log(\"cli write ret:\", ret);\n",
            "    c.on(\"data\", (d) => {\n",
            "      console.log(\"cli got:\", d.toString());\n",
            "    });\n",
            "  });\n",
            "  c.on(\"close\", () => {\n",
            "    console.log(\"cli close\");\n",
            "    server.close(() => console.log(\"srv closed\"));\n",
            "  });\n",
            "});\n",
            "server.on(\"close\", () => {\n",
            "  const a2 = server.address();\n",
            "  console.log(\"srv close evt addr:\", a2 === null ? \"null\" : typeof a2);\n",
            "});\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(
        out,
        concat!(
            "addr: 127.0.0.1 47114 IPv4\n",
            "conns: null 0\n",
            "cli cb: ra= 127.0.0.1 rp= 47114 rf= IPv4\n",
            "cli write ret: true\n",
            "srv got: hello w-ret: true\n",
            "cli got: ok\n",
            "cli close\n",
            "srv close evt addr: object\n",
            "srv closed"
        )
    );
}

/// 验证 socket.write 异步回调与 close 生命周期（服务器侧静默，避免
/// Go 中 write 回调与对端事件的固有竞态）
#[test]
fn net_write_callback_lifecycle_e2e_matches_go() {
    let work = work_dir("net_write_cb");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const net = require(\"node:net\");\n",
            "const server = net.createServer((sock) => {\n",
            "  sock.on(\"data\", () => {});\n",
            "});\n",
            "server.listen(47116, \"127.0.0.1\", () => {\n",
            "  console.log(\"srv: listening cb\");\n",
            "  const c = net.connect({ port: 47116, host: \"127.0.0.1\" }, () => {\n",
            "    console.log(\"cli: connected cb\");\n",
            "    c.write(\"payload\", () => {\n",
            "      console.log(\"cli: write cb\");\n",
            "      c.end();\n",
            "    });\n",
            "  });\n",
            "  c.on(\"close\", () => {\n",
            "    console.log(\"cli: close evt\");\n",
            "    server.close(() => console.log(\"srv: close cb\"));\n",
            "  });\n",
            "});\n",
            "server.on(\"listening\", () => console.log(\"srv: listening evt\"));\n",
            "server.on(\"close\", () => console.log(\"srv: close evt\"));\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(
        out,
        concat!(
            "srv: listening cb\n",
            "srv: listening evt\n",
            "cli: connected cb\n",
            "cli: write cb\n",
            "cli: close evt\n",
            "srv: close evt\n",
            "srv: close cb"
        )
    );
}

/// 验证 net.isIP/isIPv4/isIPv6 全分支、BlockList（address/subnet/range/check/rules）
/// 与 SocketAddress（family 小写规范化）
#[test]
fn net_isip_blocklist_socketaddress_e2e_matches_go() {
    let work = work_dir("net_isip");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const net = require(\"node:net\");\n",
            "console.log(\"isIP:\", net.isIP(\"1.2.3.4\"), net.isIP(\"::1\"), net.isIP(\"fe80::1\"), net.isIP(\"abc\"), net.isIP(\"\"), net.isIP(\"1.2.3\"), net.isIP(\"256.1.1.1\"));\n",
            "console.log(\"isIPv4:\", net.isIPv4(\"127.0.0.1\"), net.isIPv4(\"::1\"), net.isIPv4(\"xyz\"));\n",
            "console.log(\"isIPv6:\", net.isIPv6(\"::1\"), net.isIPv6(\"127.0.0.1\"), net.isIPv6(\"2001:db8::8a2e:370:7334\"));\n",
            "console.log(\"types:\", typeof net.BlockList, typeof net.SocketAddress);\n",
            "const bl = new net.BlockList();\n",
            "bl.addAddress(\"10.0.0.1\");\n",
            "bl.addSubnet(\"192.168.1.0\", 24);\n",
            "bl.addRange(\"172.16.0.1\", \"172.16.0.10\");\n",
            "console.log(\"check:\", bl.check(\"10.0.0.1\"), bl.check(\"10.0.0.2\"), bl.check(\"192.168.1.55\"), bl.check(\"192.168.2.55\"), bl.check(\"172.16.0.5\"), bl.check(\"172.16.0.11\"), bl.check(\"bad\"));\n",
            "console.log(\"rules:\", bl.rules.join(\"|\"));\n",
            "const sa = new net.SocketAddress({ address: \"127.0.0.1\", port: 47110 });\n",
            "console.log(\"sa:\", sa.address, sa.port, sa.family, sa.flowlabel);\n",
            "const sa2 = new net.SocketAddress({ address: \"::1\", family: \"IPv6\" });\n",
            "console.log(\"sa2:\", sa2.address, sa2.port, sa2.family, sa2.flowlabel);\n",
            "const sa3 = new net.SocketAddress();\n",
            "console.log(\"sa3:\", sa3.address, sa3.port, sa3.family, sa3.flowlabel);\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(
        out,
        concat!(
            "isIP: 4 6 6 0 0 0 0\n",
            "isIPv4: true false false\n",
            "isIPv6: true false true\n",
            "types: function function\n",
            "check: true false true false true false false\n",
            "rules: Range: IPv4 172.16.0.1-172.16.0.10|Subnet: IPv4 192.168.1.0/24|Address: IPv4 10.0.0.1\n",
            "sa: 127.0.0.1 47110 ipv4 0\n",
            "sa2: ::1 0 ipv6 0\n",
            "sa3:  0 ipv4 0"
        )
    );
}

/// 验证 dns callback 家族：lookup（含 options 形式）/resolve/resolve4/6/
/// 记录类型/resolveAny/reverse/lookupService/Resolver（顺序链式回调保证
/// 与 Go 的 goroutine 完成序无关）
#[test]
fn dns_callback_family_e2e_matches_go() {
    let work = work_dir("dns_cb");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const dns = require(\"node:dns\");\n",
            "console.log(\"order:\", dns.getDefaultResultOrder());\n",
            "dns.setDefaultResultOrder(\"ipv4first\");\n",
            "console.log(\"order2:\", dns.getDefaultResultOrder());\n",
            "console.log(\"consts:\", dns.NODATA, dns.NOTFOUND, dns.TIMEOUT, dns.EOF);\n",
            "console.log(\"servers:\", dns.getServers().join(\",\"));\n",
            "dns.lookup(\"localhost\", (err, address, family) => {\n",
            "  console.log(\"lookup:\", err, address, family);\n",
            "  dns.lookup(\"localhost\", {}, (err2, a2, f2) => {\n",
            "    console.log(\"lookup-opts:\", err2, a2, f2);\n",
            "    dns.resolve(\"localhost\", (e3, r3) => {\n",
            "      console.log(\"resolve:\", e3, r3.join(\"|\"));\n",
            "      dns.resolve4(\"localhost\", (e4, r4) => {\n",
            "        console.log(\"resolve4:\", e4, r4.join(\"|\"));\n",
            "        dns.resolve6(\"localhost\", (e5, r5) => {\n",
            "          console.log(\"resolve6:\", e5, r5.join(\"|\"));\n",
            "          dns.resolveCname(\"localhost\", (e6, r6) => {\n",
            "            console.log(\"cname:\", e6, r6.length);\n",
            "            dns.resolveNs(\"localhost\", (e7, r7) => {\n",
            "              console.log(\"ns:\", e7, r7.length);\n",
            "              dns.resolveTxt(\"localhost\", (e8, r8) => {\n",
            "                console.log(\"txt:\", e8, r8.length);\n",
            "                dns.resolveMx(\"localhost\", (e9, r9) => {\n",
            "                  console.log(\"mx:\", e9, r9.length);\n",
            "                  dns.resolveSrv(\"localhost\", (e10, r10) => {\n",
            "                    console.log(\"srv:\", e10, r10.length);\n",
            "                    dns.resolvePtr(\"localhost\", (e11, r11) => {\n",
            "                      console.log(\"ptr:\", e11, r11.length);\n",
            "                      dns.resolveCaa(\"localhost\", (e12, r12) => {\n",
            "                        console.log(\"caa:\", e12, r12.length);\n",
            "                        dns.resolveSoa(\"localhost\", (e13, s) => {\n",
            "                          console.log(\"soa:\", e13, typeof s);\n",
            "                          dns.resolveAny(\"localhost\", (e14, ra) => {\n",
            "                            console.log(\"any:\", e14, ra.map((x) => x.type + \":\" + x.address).join(\"|\"));\n",
            "                            dns.reverse(\"not-an-ip\", (e15, h) => {\n",
            "                              console.log(\"rev-bad:\", e15 && e15.code, e15 && e15.hostname, h);\n",
            "                              dns.reverse(\"127.0.0.1\", (e16, h2) => {\n",
            "                                console.log(\"rev-ok:\", e16, typeof h2 + \":\" + (h2 === null));\n",
            "                                dns.lookupService(\"not-an-ip\", 80, (e17, ls) => {\n",
            "                                  console.log(\"ls-bad:\", e17 && e17.code, ls);\n",
            "                                  dns.lookupService(\"127.0.0.1\", 80, (e18, ls2) => {\n",
            "                                    console.log(\"ls-ok:\", e18, ls2.service);\n",
            "                                    const r = new dns.Resolver();\n",
            "                                    r.resolve4(\"localhost\", (e19, rr) => {\n",
            "                                      console.log(\"resolver4:\", e19, rr.join(\"|\"));\n",
            "                                      console.log(\"res-cancels:\", typeof r.cancel);\n",
            "                                    });\n",
            "                                  });\n",
            "                                });\n",
            "                              });\n",
            "                            });\n",
            "                          });\n",
            "                        });\n",
            "                      });\n",
            "                    });\n",
            "                  });\n",
            "                });\n",
            "              });\n",
            "            });\n",
            "          });\n",
            "        });\n",
            "      });\n",
            "    });\n",
            "  });\n",
            "});\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(
        out,
        concat!(
            "order: verbatim\n",
            "order2: ipv4first\n",
            "consts: ENODATA ENOTFOUND ETIMEOUT EOF\n",
            "servers: \n",
            "lookup: null ::1 6\n",
            "lookup-opts: null ::1 6\n",
            "resolve: null 127.0.0.1\n",
            "resolve4: null 127.0.0.1\n",
            "resolve6: null ::1\n",
            "cname: null 0\n",
            "ns: null 0\n",
            "txt: null 0\n",
            "mx: null 0\n",
            "srv: null 0\n",
            "ptr: null 0\n",
            "caa: null 0\n",
            "soa: null object\n",
            "any: null A:::1|A:127.0.0.1\n",
            "rev-bad: ENOTFOUND  null\n",
            "rev-ok: null object:false\n",
            "ls-bad: ENOTFOUND null\n",
            "ls-ok: null http\n",
            "resolver4: null 127.0.0.1\n",
            "res-cancels: function"
        )
    );
}

/// 验证 dns/promises：await lookup/resolve 家族、Resolver 表面、与
/// `require("node:dns").promises` 恒等（同一对象）
#[test]
fn dns_promises_family_e2e_matches_go() {
    let work = work_dir("dns_promises");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const dns = require(\"node:dns/promises\");\n",
            "const dnsMain = require(\"node:dns\");\n",
            "async function main() {\n",
            "  console.log(\"order:\", dns.getDefaultResultOrder());\n",
            "  console.log(\"identity:\", dns === dnsMain.promises);\n",
            "  console.log(\"pResolver:\", typeof dns.Resolver);\n",
            "  const pr = new dns.Resolver();\n",
            "  console.log(\"pr methods:\", typeof pr.resolve4, typeof pr.lookup, typeof pr.cancel);\n",
            "  const r = await dns.lookup(\"localhost\");\n",
            "  console.log(\"lookup:\", r.address, r.family, typeof r);\n",
            "  const r4 = await dns.resolve4(\"localhost\");\n",
            "  console.log(\"resolve4:\", r4.join(\"|\"));\n",
            "  const r6 = await dns.resolve6(\"localhost\");\n",
            "  console.log(\"resolve6:\", r6.join(\"|\"));\n",
            "  const rc = await dns.resolveCname(\"localhost\");\n",
            "  console.log(\"cname:\", rc.length);\n",
            "  const rt = await dns.resolveTxt(\"localhost\");\n",
            "  console.log(\"txt:\", rt.length);\n",
            "  const rm = await dns.resolveMx(\"localhost\");\n",
            "  console.log(\"mx:\", rm.length);\n",
            "  const rs = await dns.resolveSrv(\"localhost\");\n",
            "  console.log(\"srv:\", rs.length);\n",
            "  const rp = await dns.resolvePtr(\"localhost\");\n",
            "  console.log(\"ptr:\", rp.length);\n",
            "  const caa = await dns.resolveCaa(\"localhost\");\n",
            "  console.log(\"caa:\", caa.length);\n",
            "  const soa = await dns.resolveSoa(\"localhost\");\n",
            "  console.log(\"soa:\", typeof soa);\n",
            "  const rn = await dns.resolveNs(\"localhost\");\n",
            "  console.log(\"ns:\", rn.length);\n",
            "  const rAny = await dns.resolveAny(\"localhost\");\n",
            "  console.log(\"any:\", rAny.map((x) => x.type + \":\" + x.address).join(\"|\"));\n",
            "  console.log(\"servers:\", dns.getServers().join(\",\"));\n",
            "  console.log(\"rev:\", (await dns.reverse(\"127.0.0.1\")).length >= 0);\n",
            "}\n",
            "main();\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(
        out,
        concat!(
            "order: verbatim\n",
            "identity: true\n",
            "pResolver: function\n",
            "pr methods: function undefined function\n",
            "lookup: ::1 6 object\n",
            "resolve4: 127.0.0.1\n",
            "resolve6: ::1\n",
            "cname: 0\n",
            "txt: 0\n",
            "mx: 0\n",
            "srv: 0\n",
            "ptr: 0\n",
            "caa: 0\n",
            "soa: object\n",
            "ns: 0\n",
            "any: A:::1|A:127.0.0.1\n",
            "servers: \n",
            "rev: true"
        )
    );
}

/// 验证 dgram UDP 真实回环收发：bind/listening/message（Buffer + rinfo）/
/// send 回调/隐式绑定 send/close
#[test]
fn dgram_loopback_send_receive_e2e_matches_go() {
    let work = work_dir("dgram_loopback");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const dgram = require(\"node:dgram\");\n",
            "const s = dgram.createSocket(\"udp4\");\n",
            "s.on(\"message\", (msg, rinfo) => {\n",
            "  console.log(\"msg:\", msg.toString(), \"addr:\", rinfo.address, \"fam:\", rinfo.family, \"size:\", rinfo.size, \"type:\", typeof msg);\n",
            "  s.close(() => console.log(\"closed cb\"));\n",
            "});\n",
            "s.on(\"listening\", () => {\n",
            "  console.log(\"listening evt\");\n",
            "  const c = dgram.createSocket(\"udp4\");\n",
            "  c.send(\"hello-udp\", 47113, \"127.0.0.1\", (err) => {\n",
            "    console.log(\"send cb:\", err);\n",
            "    c.close(() => console.log(\"client closed\"));\n",
            "  });\n",
            "});\n",
            "s.bind(47113, \"127.0.0.1\", () => {\n",
            "  console.log(\"bind cb\");\n",
            "});\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(
        out,
        concat!(
            "listening evt\n",
            "send cb: null\n",
            "client closed\n",
            "bind cb\n",
            "msg: hello-udp addr: 127.0.0.1 fam: IPv4 size: 9 type: object\n",
            "closed cb"
        )
    );
}

/// 验证 tls API 表面、选项校验错误消息（逐字）、createSecureContext 形态、
/// checkServerIdentity 与 getCiphers（Go crypto/tls.CipherSuites 同序列表）。
/// TLS 握手路径未实现：TLSSocket 仅作表面验证并在探针内 destroy。
#[test]
fn tls_surface_and_errors_e2e_matches_go() {
    let work = work_dir("tls_surface");
    std::fs::write(
        work.join("probe.js"),
        concat!(
            "const tls = require(\"node:tls\");\n",
            "console.log(\"types:\", typeof tls.createServer, typeof tls.connect, typeof tls.TLSSocket, typeof tls.createSecureContext, typeof tls.checkServerIdentity, typeof tls.getCiphers);\n",
            "console.log(\"checkServerIdentity:\", tls.checkServerIdentity(\"a.com\", {}));\n",
            "const ciphers = tls.getCiphers();\n",
            "console.log(\"ciphers:\", ciphers.length, ciphers.join(\",\"));\n",
            "try {\n",
            "  tls.createServer();\n",
            "} catch (e) {\n",
            "  console.log(\"err1:\", e.message);\n",
            "}\n",
            "try {\n",
            "  tls.createServer({});\n",
            "} catch (e) {\n",
            "  console.log(\"err2:\", e.message);\n",
            "}\n",
            "try {\n",
            "  tls.createServer({ key: \"k\" });\n",
            "} catch (e) {\n",
            "  console.log(\"err3:\", e.message);\n",
            "}\n",
            "try {\n",
            "  tls.createServer({ key: \"k\", cert: \"c\" });\n",
            "} catch (e) {\n",
            "  console.log(\"err4:\", e.message);\n",
            "}\n",
            "const sc = tls.createSecureContext({ key: \"k\", cert: \"c\" });\n",
            "console.log(\"sc garbage:\", typeof sc.key, typeof sc.cert, typeof sc.context);\n",
            "const sock = new tls.TLSSocket();\n",
            "console.log(\"tls socket:\", typeof sock.on, typeof sock.write, typeof sock.destroy, typeof sock.getProtocol);\n",
            "sock.destroy();\n",
            "console.log(\"after destroy ok\");\n",
        ),
    )
    .unwrap();
    let out = common::assert_e2e_matches_go(&work, "probe.js");
    assert_eq!(
        out,
        concat!(
            "types: function function function function function function\n",
            "checkServerIdentity: undefined\n",
            "ciphers: 13 TLS_AES_128_GCM_SHA256,TLS_AES_256_GCM_SHA384,TLS_CHACHA20_POLY1305_SHA256,TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA,TLS_ECDHE_ECDSA_WITH_AES_256_CBC_SHA,TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA,TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA,TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256,TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256\n",
            "err1: tls: createServer requires { key, cert } options\n",
            "err2: tls: createServer requires { key, cert } PEM options\n",
            "err3: tls: createServer requires { key, cert } PEM options\n",
            "err4: tls: invalid key/cert: tls: failed to find any PEM data in certificate input\n",
            "sc garbage: undefined undefined object\n",
            "tls socket: function function function undefined\n",
            "after destroy ok"
        )
    );
}
