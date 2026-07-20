### Metadata shown outside the application itself: the desktop launcher entry
### and the AppStream description used by software centres such as GNOME
### Software, KDE Discover and Flathub.
###
### Notes for translators:
### These strings are generated into resources/*.desktop and
### resources/*.metainfo.xml by scripts/gen-metadata.py. Do not edit those two
### files by hand.
### Product names (COSMIC, WiFi, QR) are not translated. The trademark sign in
### COSMIC™ must be kept.
### Release notes in the metainfo file are deliberately not translated.

## Desktop launcher entry.

# Application name in the launcher. Keep it short, it appears under the icon.
desktop-name = Camera
# Generic description of the application type, shown by some launchers instead
# of, or alongside, the name above.
desktop-generic-name = Camera
# One line tooltip shown in the launcher.
desktop-comment = Third-party camera app for the COSMIC™ desktop
# Search terms for the launcher, separated by semicolons and ending with one.
# Translate the terms, and feel free to add extra words people in your language
# would search for. Keeping the English words as well is useful, since many
# users type them regardless.
desktop-keywords = camera;webcam;photo;video;

## Software centre listing.

# Application name in software centres. Should match the launcher name above.
metainfo-name = Camera
# One line summary shown under the name in a software centre. Keep it under
# about 60 characters, as listings truncate it.
metainfo-summary = Capture photos and videos
# First paragraph of the long description.
metainfo-description-intro = Camera is a third-party camera application for the COSMIC™ desktop environment. Whether you need to snap a quick photo, record a video, or scan a QR code, Camera provides a clean and intuitive interface that stays out of your way.
# Second paragraph of the long description.
metainfo-description-usage = Just open the app and start capturing moments. Add fun filters to your photos, scan QR codes to open links or connect to WiFi, or use virtual camera mode to look great in video calls with your favorite filter applied.
# Heading introducing the feature list below. Ends with a colon.
metainfo-description-features-title = Key features:
# Feature list item.
metainfo-feature-capture = Photo and video capture: high quality and hardware accelerated
# Feature list item. QR and WiFi are product names and stay as they are.
metainfo-feature-qr = QR code scanner: open links, connect to WiFi, and more
# Feature list item. The filter names are the same ones used in the app, so
# translate them the same way there.
metainfo-feature-filters = 15 creative filters: Mono, Sepia, Vivid, Noir, Pencil Sketch, and more
# Feature list item.
metainfo-feature-virtual-camera = Virtual camera: use your filtered camera feed in video calls and other apps
# Feature list item.
metainfo-feature-multi-camera = Multi-camera support: easily switch between cameras and microphones

## Screenshot captions in the software centre listing.

# Caption for the screenshot of photo mode with the tools row open.
metainfo-caption-photo-tools = Photo mode with tools menu
# Caption for the screenshot taken on a mobile device.
metainfo-caption-phone = Photo mode on a Linux phone
# Caption for the screenshot of the filter picker.
metainfo-caption-filters = Filter picker
# Caption for the screenshot taken while a video is being recorded.
metainfo-caption-recording = Video recording in progress
# Caption for the screenshot showing a detected QR code.
metainfo-caption-qr = QR code detection
# Caption for the screenshot of the settings panel.
metainfo-caption-settings = Advanced settings
