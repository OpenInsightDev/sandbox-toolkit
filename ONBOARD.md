# ONBOARD

- [ ] P0：按 `crates/pty/ONBOARD.md` 补齐 pty crate 能力
- [ ] P1：服务端添加对 pty 的完整支持
- [ ] P1：sdk 实现 Skill Service，并与服务端集成测试
- [ ] P2：sdk 的 pty 附着改用 HTTP/2 extended CONNECT（RFC 8441）；服务端已支持，客户端仍走 HTTP/1.1 WebSocket upgrade
