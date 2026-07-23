#!/usr/bin/env python3
"""Inject translations from i18n/*/camera.ftl into the derived metadata files.

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

The screenshot gallery in preview/README.md is written from the same strings, so
its captions never drift from the ones Flathub shows. That file is generated as
a whole, English only, since it is repository documentation rather than
something translators are asked to maintain.

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
PREVIEW_README = ROOT / "preview" / "README.md"

SOURCE_LANG = "en"

# The application's Fluent file holds both the strings shown inside the app and
# the ones generated from here. These prefixes mark the latter.
KEY_PREFIXES = ("desktop-", "metainfo-")

# Desktop entry key -> Fluent key.
DESKTOP_KEYS = {
    "Name": "camera",
    "GenericName": "camera",
    "Comment": "desktop-comment",
    "Keywords": "desktop-keywords",
}

# Metainfo elements, in document order within their region, mapped to Fluent
# keys. The nth English (untagged) element of each kind takes the nth key.
METAINFO_HEAD = {
    "name": ["camera"],
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
        "metainfo-feature-modes",
        "metainfo-feature-controls",
        "metainfo-feature-qr",
        "metainfo-feature-filters",
        "metainfo-feature-virtual-camera",
        "metainfo-feature-multi-camera",
    ],
}
# Appearance combinations captured by preview/capture-previews.sh. Keep in sync
# with VARIANTS and PUBLISHED_VARIANT there. The published variant is stored as
# the plain preview-0NN.png that metainfo.xml points at, not under variants/.
VARIANT_THEMES = ["dark", "light"]
VARIANT_OVERLAYS = ["frosted", "translucent", "off"]
PUBLISHED_VARIANT = "dark-translucent"

# Where capture-previews.sh puts one published shot per translated language.
LOCALES_DIR = "locales"

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
    """Return {fluent_key: {lang: value}} for every string of every language.

    In-app keys are read too, not just the metadata ones: the metadata maps
    some of them directly (`camera` becomes the launcher name) and references
    others inside its own strings, so they all have to be resolvable.
    """
    out: dict[str, dict[str, str]] = {}
    for path in sorted(I18N.glob("*/camera.ftl")):
        lang = path.parent.name
        for line in path.read_text(encoding="utf-8").splitlines():
            match = re.match(r"^([a-z0-9-]+) = (.*)$", line)
            if match:
                out.setdefault(match.group(1), {})[lang] = match.group(2).strip()
    return out


def resolve_references(translations: dict[str, dict[str, str]]) -> None:
    """Expand { key } placeables in place, falling back to the source language.

    Fluent resolves these itself for the strings the application renders, but
    the desktop and metainfo files are plain text, so they need the final value.
    """
    reference = re.compile(r"\{\s*([a-z0-9-]+)\s*\}")

    def expand(key: str, lang: str, seen: frozenset[str]) -> str:
        if key in seen:
            sys.exit(f"error: circular reference to {{ {key} }} in i18n/{lang}/camera.ftl")
        values = translations.get(key)
        if not values:
            sys.exit(f"error: unknown reference {{ {key} }} in i18n/{lang}/camera.ftl")
        value = values.get(lang, values.get(SOURCE_LANG, ""))
        return reference.sub(
            lambda m: expand(m.group(1), lang, seen | {key}), value
        )

    for key, values in translations.items():
        for lang in values:
            values[lang] = expand(key, lang, frozenset())


def langs_for(translations: dict[str, dict[str, str]], key: str) -> list[str]:
    """Translated languages for a key, source language excluded, sorted."""
    return sorted(lang for lang in translations.get(key, {}) if lang != SOURCE_LANG)


def render_desktop(translations: dict[str, dict[str, str]], text: str) -> str:
    """Rewrite the mapped values of a desktop entry, English line included.

    The metainfo renderer rewrites its English elements too, so doing the same
    here keeps both files fully generated. Anything unmapped (Icon, Exec, and
    the rest) is copied through untouched.
    """
    # Desktop entries use POSIX locale names, so zh-CN becomes zh_CN.
    lines = [ln for ln in text.splitlines() if not re.match(r"^\w+\[[^\]]+\]=", ln)]
    out: list[str] = []
    for line in lines:
        match = re.match(r"^(\w+)=", line)
        if not match or match.group(1) not in DESKTOP_KEYS:
            out.append(line)
            continue
        key = DESKTOP_KEYS[match.group(1)]
        out.append(f"{match.group(1)}={translations[key][SOURCE_LANG]}")
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
    text = text[: shots[0]] + render_screenshot_images(render_region(
        text[shots[0]:shots[1]], METAINFO_SCREENSHOTS, translations
    )) + text[shots[1]:]
    text = text[: desc[0]] + render_region(
        text[desc[0]:desc[1]], METAINFO_DESCRIPTION, translations
    ) + text[desc[1]:]
    text = render_region(text[:head_end], METAINFO_HEAD, translations) + text[head_end:]
    return text


def localized_images(image: str) -> dict[str, str]:
    """Languages that have their own capture of one screenshot, to its URL.

    Driven by what preview/capture-previews.sh actually produced rather than by
    the language list, so a language whose shots have not been taken yet simply
    has no localized screenshot instead of a link to a missing file.
    """
    name = Path(image).name
    found = {}
    for path in sorted((PREVIEW_README.parent / LOCALES_DIR).glob(f"*/{name}")):
        found[path.parent.name] = f"{LOCALES_DIR}/{path.parent.name}/{name}"
    return found


def render_screenshot_images(region: str) -> str:
    """Give each <image> an xml:lang sibling per language captured for it.

    AppStream picks the <image> matching the user's language and falls back to
    the untagged one, so a browser in German shows German screenshots while
    everyone else still sees the English originals.

    Same group shape as render_region: the English element plus the xml:lang
    siblings that follow it are replaced together, so re-running is idempotent
    and a language whose captures were deleted loses its entries again.
    """
    pattern = re.compile(
        r'^([ \t]*)<image(?:\s+xml:lang="[^"]+")?>(.*?)</image>', re.S | re.M
    )
    groups: list[list] = []  # [start, end, indent, english url]
    for match in pattern.finditer(region):
        if "xml:lang" not in match.group(0).split(">", 1)[0]:
            groups.append([match.start(), match.end(), match.group(1),
                           match.group(2).strip()])
        elif groups:
            groups[-1][1] = match.end()

    for start, end, indent, url in reversed(groups):
        parts = [f"{indent}<image>{escape(url)}</image>"]
        base = url.rsplit("/", 1)[0]
        for lang, rel in localized_images(url).items():
            parts.append(
                f'{indent}<image xml:lang="{lang}">{escape(f"{base}/{rel}")}</image>'
            )
        region = region[:start] + "\n".join(parts) + region[end:]
    return region


def screenshot_images(metainfo: str) -> list[str]:
    """File names of the <image> elements, in document order."""
    start, end = span(metainfo, "screenshots")
    return [
        Path(match.group(1).strip()).name
        for match in re.finditer(r"<image>(.*?)</image>", metainfo[start:end], re.S)
    ]


def variant_image(image: str, theme: str, overlay: str) -> str:
    """Path of one appearance variant of a published screenshot."""
    variant = f"{theme}-{overlay}"
    if variant == PUBLISHED_VARIANT:
        return image
    return f"variants/{Path(image).stem}-{variant}.png"


def render_variant_gallery(translations: dict[str, dict[str, str]],
                           images: list[str], keys: list[str]) -> list[str]:
    """A theme x overlay-effect table for each screenshot.

    Variants that have not been captured are skipped rather than linked, so a
    partial run produces a shorter page instead of a page full of broken images.
    """
    lines: list[str] = []
    for image, key in zip(images, keys):
        available = {
            (theme, overlay): variant_image(image, theme, overlay)
            for theme in VARIANT_THEMES
            for overlay in VARIANT_OVERLAYS
            if (PREVIEW_README.parent / variant_image(image, theme, overlay)).exists()
        }
        if not available:
            continue

        caption = translations[key][SOURCE_LANG]
        overlays = [o for o in VARIANT_OVERLAYS
                    if any((t, o) in available for t in VARIANT_THEMES)]

        lines += [
            "",
            f"### {caption}",
            "",
            "|  | " + " | ".join(o.capitalize() for o in overlays) + " |",
            "| :--- |" + " :---: |" * len(overlays),
        ]
        for theme in VARIANT_THEMES:
            if not any((theme, o) in available for o in overlays):
                continue
            cells = []
            for overlay in overlays:
                path = available.get((theme, overlay))
                cells.append(
                    f"![{caption}, {theme} {overlay}]({path})" if path else ""
                )
            lines.append(f"| **{theme.capitalize()}** | " + " | ".join(cells) + " |")
    return lines


def captured_languages(images: list[str]) -> dict[str, dict[str, str]]:
    """Language -> {published image: its localized counterpart}."""
    by_lang: dict[str, dict[str, str]] = {}
    for image in images:
        for lang, rel in localized_images(image).items():
            by_lang.setdefault(lang, {})[image] = rel
    return by_lang


def render_locale_gallery(images: list[str], keys: list[str],
                          translations: dict[str, dict[str, str]]) -> list[str]:
    """An index of the per-language galleries, one row each.

    Only an index: embedding sixty screenshots here would bury the published
    gallery this page exists for. Each language's own page carries its shots.
    """
    by_lang = captured_languages(images)
    if not by_lang:
        return []

    def entry(lang: str) -> str:
        # The app's own name in that language doubles as a preview of the
        # translation, so the list shows what the reader is clicking into.
        name = translations["camera"].get(lang, translations["camera"][SOURCE_LANG])
        done = sum(1 for key in keys if lang in translations[key])
        shots = len(by_lang[lang])
        # A leading Left-to-Right Mark keeps the list left aligned. Without it,
        # a right-to-left native name (Arabic here) sets the base direction and
        # GitHub right aligns every bullet in the list.
        return (f"- &lrm;[**{name}** (`{lang}`)]({LOCALES_DIR}/{lang}/README.md)"
                f" &mdash; {shots} shot{'s' if shots != 1 else ''},"
                f" {done}/{len(keys)} captions translated")

    return [
        "",
        "---",
        "",
        "## Languages",
        "",
        "The published shots in every language the app is translated into, including the",
        "partly translated ones: untranslated labels stay in English and show what is left",
        f"to do. Only {SOURCE_LANG} gets the full appearance matrix above.",
        "",
        *(entry(lang) for lang in sorted(by_lang)),
        "",
    ]


def render_locale_readme(lang: str, images: list[str], keys: list[str],
                         translations: dict[str, dict[str, str]]) -> str:
    """The gallery page that sits beside one language's screenshots.

    Captions fall back to English per string rather than per page, so a partly
    translated language reads the same way its screenshots look: translated
    where it has been done, English where it has not.
    """
    def text(key: str) -> str:
        return translations[key].get(lang, translations[key][SOURCE_LANG])

    cells = []
    for image, key in zip(images, keys):
        rel = localized_images(image).get(lang)
        if not rel:
            continue
        caption = text(key).replace("|", "\\|")
        alt = caption.replace("]", "\\]")
        # The page lives in the same directory as the images it shows.
        cells.append(f"![{alt}]({Path(rel).name})<br>**{caption}**")

    rows = [cells[i:i + 2] for i in range(0, len(cells), 2)]
    missing = [key for key in keys if lang not in translations[key]]

    return "\n".join([
        f"<!-- Generated by scripts/{Path(__file__).name}. Edit the captions in "
        f"i18n/{lang}/camera.ftl and run `just generate`. -->",
        "",
        f"# {text('camera')} ({lang})",
        "",
        f"*{text('metainfo-summary')}.*",
        "",
        "|  |  |",
        "| :---: | :---: |",
        *(f"| {' | '.join(row + [''] * (2 - len(row)))} |" for row in rows),
        "",
        *([
            f"> {len(missing)} of {len(keys)} captions are not translated into `{lang}` yet",
            "> and are shown in English. Translations are welcome in",
            f"> [`i18n/{lang}/camera.ftl`](../../../i18n/{lang}/camera.ftl).",
            "",
        ] if missing else []),
        "---",
        "",
        f"[All languages](../../README.md#languages) ·",
        f"[{SOURCE_LANG} screenshots, including every theme and overlay effect](../../README.md)",
        "",
    ])


def render_preview_readme(translations: dict[str, dict[str, str]], metainfo: str) -> str:
    """A two column gallery pairing each metainfo screenshot with its caption.

    The images come from the metainfo file rather than from a listing of the
    directory, so the page shows exactly the screenshots that are published and
    in the same order.
    """
    keys = METAINFO_SCREENSHOTS["caption"]
    images = screenshot_images(metainfo)
    if len(images) != len(keys):
        sys.exit(
            f"error: expected {len(keys)} <image> elements, found {len(images)}. "
            f"Update the mapping in {Path(__file__).name}."
        )

    def cell(image: str, key: str) -> str:
        caption = translations[key][SOURCE_LANG].replace("|", "\\|")
        alt = caption.replace("]", "\\]")
        return f"![{alt}]({image})<br>**{caption}**"

    cells = [cell(image, key) for image, key in zip(images, keys)]
    rows = [cells[i:i + 2] for i in range(0, len(cells), 2)]

    return "\n".join([
        f"<!-- Generated by scripts/{Path(__file__).name}. Edit the captions in "
        f"i18n/{SOURCE_LANG}/camera.ftl and run `just generate`. -->",
        "",
        f"# {translations['camera'][SOURCE_LANG]} screenshots",
        "",
        f"*{translations['metainfo-summary'][SOURCE_LANG]}.* The gallery published with the",
        "application metadata and shown on",
        "[Flathub](https://flathub.org/apps/io.github.cosmic_utils.camera).",
        "",
        "|  |  |",
        "| :---: | :---: |",
        *(f"| {' | '.join(row + [''] * (2 - len(row)))} |" for row in rows),
        "",
        "---",
        "",
        "## Appearance variants",
        "",
        "Each shot in every combination of theme and overlay effect the app offers.",
        f"The published screenshots above are the {PUBLISHED_VARIANT.replace('-', ', ')} ones.",
        *render_variant_gallery(translations, images, keys),
        *render_locale_gallery(images, keys, translations),
        "",
        "---",
        "",
        "To retake them, run [`generate-previews.sh`](generate-previews.sh), which drives the app",
        "against a fixed image feed inside a pinned container so every shot is reproducible.",
        "",
        "[Back to the project README](../README.md)",
        "",
    ])


def write_if_changed(path: Path, content: str) -> bool:
    # The per-language pages are created as their screenshots appear, so a
    # missing file is normal here rather than a sign the tree is broken.
    if not path.exists():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
        print(f"  created    {path.relative_to(ROOT)}")
        return True
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

    expected = set(DESKTOP_KEYS.values())
    for mapping in (METAINFO_HEAD, METAINFO_DESCRIPTION, METAINFO_SCREENSHOTS):
        expected.update(k for keys in mapping.values() for k in keys)
    # Only the prefixed keys and the mapped ones are this script's business;
    # the rest of the file is the application's own strings.
    source_keys = {k for k, v in translations.items() if SOURCE_LANG in v
                   and (k.startswith(KEY_PREFIXES) or k in expected)}
    if missing := expected - source_keys:
        sys.exit(f"error: missing from i18n/{SOURCE_LANG}/camera.ftl: {', '.join(sorted(missing))}")
    if extra := source_keys - expected:
        sys.exit(f"error: unmapped keys in i18n/{SOURCE_LANG}/camera.ftl: {', '.join(sorted(extra))}")

    resolve_references(translations)

    langs = sorted({lang for v in translations.values() for lang in v
                    if lang != SOURCE_LANG and any(
                        lang in translations[k] for k in expected)})
    print(f"Generating metadata for: {', '.join(langs)}")

    write_if_changed(DESKTOP, render_desktop(translations, DESKTOP.read_text(encoding="utf-8")))
    metainfo = render_metainfo(translations, METAINFO.read_text(encoding="utf-8"))
    write_if_changed(METAINFO, metainfo)
    write_if_changed(PREVIEW_README, render_preview_readme(translations, metainfo))

    # One gallery page per language, beside that language's screenshots. Driven
    # by what was captured, so languages are picked up automatically and a
    # language whose shots were removed loses its stale page too.
    images = screenshot_images(metainfo)
    keys = METAINFO_SCREENSHOTS["caption"]
    locales_root = PREVIEW_README.parent / LOCALES_DIR
    captured = captured_languages(images)
    for lang in sorted(captured):
        write_if_changed(locales_root / lang / "README.md",
                         render_locale_readme(lang, images, keys, translations))
    for stale in sorted(locales_root.glob("*/README.md")):
        if stale.parent.name not in captured:
            stale.unlink()
            print(f"  removed    {stale.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
