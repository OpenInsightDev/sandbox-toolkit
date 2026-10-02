# tus 1.0.0 协议（本 crate 相关节选）

官方文档：<https://tus.io/protocols/resumable-upload>
版本：1.0.0（[SemVer](https://semver.org)），日期 2016-03-25。

本目录只收录本 crate 需要实现的部分，正文为官方规范原文，按章节拆分为独立文件。

## 目录

- [Core Protocol](./core-protocol.md)
- [Creation](./creation.md)
- [Creation With Upload](./creation-with-upload.md)
- [Termination](./termination.md)
- [Concatenation](./concatenation.md)

## About this version

**Version:** 1.0.0 ([SemVer](https://semver.org))
**Date:** 2016-03-25

**Authors:** [Felix Geisendörfer](https://twitter.com/felixge), [Kevin van Zonneveld](https://twitter.com/kvz), [Tim Koschützki](https://twitter.com/tim_kos), [Naren Venkataraman](https://github.com/vayam), [Marius Kleidl](https://twitter.com/Acconut_)

**Collaborators**: [Bruno de Carvalho](https://github.com/biasedbit), [James Butler](https://github.com/sandfox), [Øystein Steimler](https://github.com/cybic), [Sam Rijs](https://github.com/srijs), [Khang Toh](https://github.com/khangtoh), [Jacques Boscq](https://github.com/Amodio), [Jérémy FRERE](https://github.com/jerefrer), [Pieter Hintjens](https://github.com/hintjens), [Stephan Seidt](https://github.com/ehd), [Aran Wilkinson](https://github.com/aranw), [Svein Ove Aas](https://github.com/Baughn), [Oliver Anan](https://github.com/noptic), [Tim](https://github.com/schmerg), [j4james](https://github.com/j4james), [Julian Reschke](https://github.com/reschke), [Evert Pot](https://github.com/evert), [Jochen Kupperschmidt](https://github.com/homeworkprod), [Andrew Fenn](https://github.com/andrewfenn), [Kevin Swiber](https://github.com/kevinswiber), [Jan Kohlhof](https://github.com/0x20h), [eno](https://github.com/radiospiel), [Luke Arduini](https://github.com/luk-), [Jim Schmid](https://github.com/sheeep), [Jeffrey ‘jf’ Lim](https://github.com/jf), [Daniel Lopretto](https://github.com/timemachine3030), [Mark Murphy](https://github.com/MarkMurphy), [Peter Darrow](https://github.com/pmdarrow), [Gargaj](https://github.com/Gargaj), [Tomasz Rydzyński](https://github.com/qsorix), [Tino de Bruijn](https://github.com/tino), [Jonas mg](https://github.com/kless), [Christian Ulbrich](https://github.com/ChristianUlbrich), [Jon Gjengset](https://github.com/jonhoo), [Michael Elovskikh](https://github.com/wronglink), [Rick Olson](https://github.com/technoweenie), [J. Ryan Stinnett](https://convolv.es), [Ifedapo Olarewaju](https://github.com/ifedapoolarewaju), [Robert Nagy](https://github.com/ronag)

The key words “MUST”, “MUST NOT”, “REQUIRED”, “SHALL”, “SHALL NOT”, “SHOULD”, “SHOULD NOT”, “RECOMMENDED”, “MAY”, and “OPTIONAL” in this document are to be interpreted as described in [RFC 2119](https://www.rfc-editor.org/rfc/rfc2119.html).

## Status

Following [SemVer](https://semver.org), as of 1.0.0 tus is ready for general adoption. We don’t expect to make breaking changes, but if we do, those will have to be in a 2.0.0. Introducing a new extension or any backwards-compatible change adding new functionality will result in a bumped MINOR version.

## Contributing

This protocol is authored and owned by the tus community. We welcome patches and feedback via [GitHub](https://github.com/tus/tus-resumable-upload-protocol). All authors and collaborators will be listed as such in the protocol header.

Please also [let us know](https://github.com/tus/tus.io/issues/new) about any implementations (open source or commercial) if you’d like to be listed on the [implementations](https://www.tus.io/implementations.html) page.

## Abstract

The protocol provides a mechanism for resumable file uploads via HTTP ([RFC 9110](https://www.rfc-editor.org/rfc/rfc9110.html)). The protocol does not depend on specific versions of HTTP.

## Notation

Characters enclosed by square brackets indicate a placeholder (e.g. `[size]`).

The terms space, comma, and semicolon refer to their ASCII representations.
