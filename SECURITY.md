# Security policy

PixPanel runs as root and manages other people's sites, so security reports get priority over everything else.

## Reporting a vulnerability

Please do **not** open a public issue. Use GitHub's private reporting instead: the **Security** tab of this repository → **Report a vulnerability**. Include the version (`pixpanel --version`), the steps to reproduce and what an attacker gains.

You will get a first answer within a few days. Fixes ship as a new signed release; panels pick it up from Settings → Panel version.

## Supported versions

Only the latest release receives security fixes.

## Past reviews

- [`docs/infra/guvenlik-incelemesi-2026-11.md`](docs/infra/guvenlik-incelemesi-2026-11.md) (Turkish): full code review after the first release, the issues found, how each was fixed and what remains open.

---

**Türkçe:** Açık bildirmek için herkese açık kayıt açmayın; deponun **Security** sekmesindeki **Report a vulnerability** düğmesini kullanın. Yalnızca son sürüm güvenlik düzeltmesi alır.
