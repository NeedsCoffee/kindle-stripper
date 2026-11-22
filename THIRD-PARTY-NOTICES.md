# Third-party notices

Kindle-stripper is licensed under GPL-3.0-only. See [LICENSE](LICENSE). The Rust adaptation was first published on 21 November 2026. The table records which upstream files informed the port. Links identify the upstream revision inspected for this notice.

| Rust code | Upstream source and changes |
| --- | --- |
| `src/kindle_store.rs` | Adapted the Windows MSIX storage and registration code from [kindlekey.py](https://github.com/Satsuoni/DeDRM_tools/blob/ca951629926a6b06282a632c2c6eae5fe3887677/DeDRM_plugin/kindlekey.py) and [msix_registration.py](https://github.com/Satsuoni/DeDRM_tools/blob/ca951629926a6b06282a632c2c6eae5fe3887677/DeDRM_plugin/msix_registration.py) to use Windows APIs directly from Rust. |
| `src/voucher.rs` and `src/voucher_mixer.rs` | Ported the supported desktop voucher route and arithmetic from [voucher_route.py](https://github.com/Satsuoni/DeDRM_tools/blob/ca951629926a6b06282a632c2c6eae5fe3887677/DeDRM_plugin/kfx_stages/drmsdk/voucher_route.py), [win_pure_math.py](https://github.com/Satsuoni/DeDRM_tools/blob/ca951629926a6b06282a632c2c6eae5fe3887677/DeDRM_plugin/kfx_stages/drmsdk/win_pure_math.py), and [ion.py](https://github.com/Satsuoni/DeDRM_tools/blob/ca951629926a6b06282a632c2c6eae5fe3887677/DeDRM_plugin/ion.py), with a Rust keyset parser and per-book voucher lookup. |
| `src/drmion.rs` and `src/ion.rs` | Adapted the relevant binary Ion and DRMION parsing from [ion.py](https://github.com/Satsuoni/DeDRM_tools/blob/ca951629926a6b06282a632c2c6eae5fe3887677/DeDRM_plugin/ion.py) into bounded Rust readers. |

## DeDRM notices

- DeDRM plugin: Copyright © 2008–2020 Apprentice Harper et al. and Copyright © 2021–2023 NoDRM. Released under GNU GPL version 3. [Plugin notice](https://github.com/Satsuoni/DeDRM_tools/blob/ca951629926a6b06282a632c2c6eae5fe3887677/DeDRM_plugin/__init__.py#L7-L15)
- `kindlekey.py`: Copyright © 2008–2022 Apprentice Harper et al. GNU GPL version 3. [Source notice](https://github.com/Satsuoni/DeDRM_tools/blob/ca951629926a6b06282a632c2c6eae5fe3887677/DeDRM_plugin/kindlekey.py#L5-L7)
- `ion.py`: Copyright © 2013–2020 Apprentice Harper et al. GNU GPL version 3. The history credits lulzkabulz for the Pascal implementation, apprenticenaomi for the Python translation, anon for DeDRM integration, and apprenticesakuya for VoucherEnvelope v2/v3 support. [Source notice and history](https://github.com/Satsuoni/DeDRM_tools/blob/ca951629926a6b06282a632c2c6eae5fe3887677/DeDRM_plugin/ion.py#L4-L30)

The three newer files listed above (`msix_registration.py`, `voucher_route.py`, and `win_pure_math.py`) carry no separate license header in the inspected revision. Their inclusion in the GPL v3 DeDRM plugin is the basis for treating the ported portions under GPL v3.

## Rust dependencies

The Windows binary archive includes the locked crates' license and notice files under `third-party-licenses/`, with a manifest of package versions and declared licenses. The accompanying source archive includes those crates' original source trees and notices under `vendor/`. See the release page for the matching source download.
