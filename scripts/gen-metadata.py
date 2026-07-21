#!/usr/bin/env python3
"""Inject translations from i18n/*/camera.ftl into the desktop and metainfo files.

Weblate has no native format for freedesktop desktop entries or AppStream
metainfo XML, so the strings are hosted as Fluent (the same format the
application itself uses) and written into the two resource files here. They
share the application's own Fluent file because a Weblate file mask takes a
single language placeholder, so a second file would mean a second component
that translators have to find on their own.

Both resource files are edited in place and stay committed, because the release
workflow also mutates the metainfo file to add release entries. Only the
translated values are touched; release notes, versions and everything else are
left exactly as they are.

Run via `just generate`.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path
from xml.sax.saxutils import escape

ROOT = Path(__file__).resolve().parent.parent
I18N = ROOT / "i18n"
DESKTOP = ROOT / "resources" / "io.github.cosmic_utils.camera.desktop"
METAINFO = ROOT / "resources" / "io.github.cosmic_utils.camera.metainfo.xml"

SOURCE_LANG = "en"

# The application's Fluent file holds both the strings shown inside the app and
# the ones generated from here. These prefixes mark the latter.
KEY_PREFIXES = ("desktop-", "metainfo-")

# Desktop entry key -> Fluent key.
DESKTOP_KEYS = {
    "Name": "desktop-name",
    "GenericName": "desktop-generic-name",
    "Comment": "desktop-comment",
    "Keywords": "desktop-keywords",
}

# Metainfo elements, in document order within their region, mapped to Fluent
# keys. The nth English (untagged) element of each kind takes the nth key.
METAINFO_HEAD = {
    "name": ["metainfo-name"],
    "summary": ["metainfo-summary"],
}
METAINFO_DESCRIPTION = {
    "p": [
        "metainfo-description-intro",
        "metainfo-description-usage",
        "metainfo-description-features-title",
    ],
    "li": [
        "metainfo-feature-capture",
        "metainfo-feature-qr",
        "metainfo-feature-filters",
        "metainfo-feature-virtual-camera",
        "metainfo-feature-multi-camera",
    ],
}
METAINFO_SCREENSHOTS = {
    "caption": [
        "metainfo-caption-photo-tools",
        "metainfo-caption-phone",
        "metainfo-caption-filters",
        "metainfo-caption-recording",
        "metainfo-caption-qr",
        "metainfo-caption-settings",
    ],
}


def read_translations() -> dict[str, dict[str, str]]:
    """Return {fluent_key: {lang: value}} for the metadata keys of every language."""
    out: dict[str, dict[str, str]] = {}
    for path in sorted(I18N.glob("*/camera.ftl")):
        lang = path.parent.name
        for line in path.read_text(encoding="utf-8").splitlines():
            match = re.match(r"^([a-z0-9-]+) = (.*)$", line)
            if match and match.group(1).startswith(KEY_PREFIXES):
                out.setdefault(match.group(1), {})[lang] = match.group(2).strip()
    return out


def langs_for(translations: dict[str, dict[str, str]], key: str) -> list[str]:
    """Translated languages for a key, source language excluded, sorted."""
    return sorted(lang for lang in translations.get(key, {}) if lang != SOURCE_LANG)


def render_desktop(translations: dict[str, dict[str, str]], text: str) -> str:
    """Rewrite the localised Key[lang]= lines of a desktop entry."""
    # Desktop entries use POSIX locale names, so zh-CN becomes zh_CN.
    lines = [ln for ln in text.splitlines() if not re.match(r"^\w+\[[^\]]+\]=", ln)]
    out: list[str] = []
    for line in lines:
        out.append(line)
        match = re.match(r"^(\w+)=", line)
        if not match or match.group(1) not in DESKTOP_KEYS:
            continue
        key = DESKTOP_KEYS[match.group(1)]
        for lang in langs_for(translations, key):
            out.append(f"{match.group(1)}[{lang.replace('-', '_')}]={translations[key][lang]}")
    return "\n".join(out) + "\n"


def render_element_group(tag: str, indent: str, values: dict[str, str]) -> str:
    """The English element followed by one sibling per translated language."""
    parts = [f"{indent}<{tag}>{escape(values[SOURCE_LANG])}</{tag}>"]
    for lang in sorted(lang for lang in values if lang != SOURCE_LANG):
        parts.append(f'{indent}<{tag} xml:lang="{lang}">{escape(values[lang])}</{tag}>')
    return f"\n{indent}".join(part.lstrip() if i else part for i, part in enumerate(parts))


def render_region(region: str, mapping: dict[str, list[str]],
                  translations: dict[str, dict[str, str]]) -> str:
    """Rewrite every mapped element group in one region of the document.

    A group is an English element plus the xml:lang siblings that follow it.
    Only the span from the English element to its last sibling is replaced, so
    the surrounding indentation and any untouched markup stay as they are.
    """
    for tag, keys in mapping.items():
        pattern = re.compile(
            rf'^([ \t]*)<{tag}(?:\s+xml:lang="[^"]+")?>(.*?)</{tag}>', re.S | re.M
        )
        groups: list[list] = []  # [start, end, indent]
        for match in pattern.finditer(region):
            if "xml:lang" not in match.group(0).split(">", 1)[0]:
                groups.append([match.start(), match.end(), match.group(1)])
            elif groups:
                groups[-1][1] = match.end()

        if len(groups) != len(keys):
            sys.exit(
                f"error: expected {len(keys)} <{tag}> elements, found {len(groups)}. "
                f"Update the mapping in {Path(__file__).name}."
            )

        for (start, end, indent), key in reversed(list(zip(groups, keys))):
            region = region[:start] + render_element_group(
                tag, indent, translations[key]
            ) + region[end:]
    return region


def span(text: str, tag: str) -> tuple[int, int]:
    match = re.search(rf"<{tag}>.*?</{tag}>", text, re.S)
    if not match:
        sys.exit(f"error: no <{tag}> element in {METAINFO.name}")
    return match.start(), match.end()


def render_metainfo(translations: dict[str, dict[str, str]], text: str) -> str:
    """Rewrite the translated elements of the metainfo file, in place.

    The <releases> block is never touched: release notes are changelogs and are
    deliberately left untranslated. <developer> is skipped too, since the name
    inside it is a person's name.
    """
    shots = span(text, "screenshots")
    desc = span(text, "description")
    # The head region is everything before <description>, minus <developer>.
    dev = re.search(r"<developer>.*?</developer>", text[: desc[0]], re.S)
    head_end = dev.start() if dev else desc[0]

    # Rewrite back to front so earlier offsets stay valid.
    text = text[: shots[0]] + render_region(
        text[shots[0]:shots[1]], METAINFO_SCREENSHOTS, translations
    ) + text[shots[1]:]
    text = text[: desc[0]] + render_region(
        text[desc[0]:desc[1]], METAINFO_DESCRIPTION, translations
    ) + text[desc[1]:]
    text = render_region(text[:head_end], METAINFO_HEAD, translations) + text[head_end:]
    return text


def write_if_changed(path: Path, content: str) -> bool:
    if path.read_text(encoding="utf-8") == content:
        print(f"  unchanged  {path.relative_to(ROOT)}")
        return False
    path.write_text(content, encoding="utf-8")
    print(f"  updated    {path.relative_to(ROOT)}")
    return True


def main() -> int:
    translations = read_translations()
    if not translations:
        sys.exit(f"error: no metadata strings found in {I18N.relative_to(ROOT)}/*/camera.ftl")

    source_keys = {k for k, v in translations.items() if SOURCE_LANG in v}
    expected = set(DESKTOP_KEYS.values())
    for mapping in (METAINFO_HEAD, METAINFO_DESCRIPTION, METAINFO_SCREENSHOTS):
        expected.update(k for keys in mapping.values() for k in keys)
    if missing := expected - source_keys:
        sys.exit(f"error: missing from i18n/{SOURCE_LANG}/camera.ftl: {', '.join(sorted(missing))}")
    if extra := source_keys - expected:
        sys.exit(f"error: unmapped keys in i18n/{SOURCE_LANG}/camera.ftl: {', '.join(sorted(extra))}")

    langs = sorted({lang for v in translations.values() for lang in v if lang != SOURCE_LANG})
    print(f"Generating metadata for: {', '.join(langs)}")

    write_if_changed(DESKTOP, render_desktop(translations, DESKTOP.read_text(encoding="utf-8")))
    write_if_changed(METAINFO, render_metainfo(translations, METAINFO.read_text(encoding="utf-8")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
