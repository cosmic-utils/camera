app-title = Camera
about = About
repository = Repository
view = View
welcome = Welcome to COSMIC! ✨
page-id = Page { $num }
git-description = Git commit {$hash} on {$date}

# Mode switcher
mode-video = VIDEO
mode-photo = PHOTO
mode-virtual = VIRTUAL

# Virtual camera
virtual-camera-title = Virtual camera (experimental)
virtual-camera-description = Stream your camera feed to other applications via a virtual camera device. Requires PipeWire.
virtual-camera-enable = Enable virtual camera
streaming-live = LIVE
virtual-camera-open-file = Open file
virtual-camera-file-filter-name = Images and Videos

# Filters
filters-title = Filters

# Settings
settings-title = Settings
settings-appearance = Appearance
settings-theme = Theme
match-desktop = Match Desktop
dark = Dark
light = Light
settings-camera = Camera
settings-video = Video
settings-device = Device
settings-backend = Backend
settings-format = Format
settings-microphone = Microphone
settings-record-audio = Record audio
settings-audio-encoder = Audio encoder
settings-encoder = Encoder
settings-quality = Quality
settings-video-encoder = Video encoder
settings-video-quality = Video quality
settings-manual-override = Manual mode override
settings-mirror-preview = Mirror preview
settings-mirror-preview-description = Flip the camera preview horizontally
settings-reset-all = Reset all settings
settings-bug-reports = Bug reports
settings-stats-for-nerds = Stats for nerds
settings-report-bug = Report bug
settings-show-report = Show Report
settings-resolution = Resolution
settings-version = Version { $version }
settings-version-flatpak = Version { $version } (Flatpak)

# Device info
device-info-card = Card
device-info-driver = Driver
device-info-path = Path
device-info-real-path = Real Path
device-info-device-path = Device Path
device-info-sensor = Sensor
device-info-pipeline = Pipeline
device-info-libcamera-version = libcamera
device-info-multistream = Multi-stream
device-info-multistream-yes = Supported
device-info-multistream-no = Not supported
device-info-rotation = Rotation
device-info-none = No device information available

# Bitrate presets
preset-low = Low
preset-medium = Medium
preset-high = High

# Camera preview
initializing-camera = Initializing camera...

# Format picker
format-resolution = Resolution:
format-framerate = Frame Rate:

# Status indicators
indicator-res = RES
indicator-fps = FPS
indicator-hd = HD
indicator-sd = SD
indicator-4k = 4K
indicator-720p = 720p

# QR code actions
qr-open-link = Open Link
qr-connect-wifi = Connect to WiFi
qr-copy-text = Copy Text
qr-call = Call
qr-send-email = Send Email
qr-send-sms = Send SMS
qr-open-map = Open Map
qr-add-contact = Add Contact
qr-add-event = Add Event

# Exposure controls
exposure-mode = Mode
exposure-ev = EV
exposure-time = Time
exposure-gain = Gain
exposure-iso = ISO
exposure-metering = Metering
exposure-auto-priority = Frame Rate
exposure-no-controls = No exposure controls available
exposure-title = Exposure
exposure-reset = Reset
exposure-backlight = Backlight
exposure-manual-mode = Manual
exposure-auto-mode = Auto
exposure-not-supported = unsupported

# Focus controls
focus-auto = Focus
focus-position = Focus

# Color controls
color-title = Color
color-contrast = Contrast
color-saturation = Saturation
color-sharpness = Sharpness
color-hue = Hue
color-white-balance = White Balance
color-temperature = Temp
color-auto = Auto
color-manual = Manual

# Tools menu
tools-timer = Timer
tools-aspect = Aspect
tools-exposure = Exposure
tools-color = Color
tools-filter = Filter
tools-theatre = Theatre

# PTZ controls
ptz-title = Camera Controls

# Privacy cover warning
privacy-cover-closed = Privacy cover is closed
privacy-cover-hint = Open the privacy cover to use the camera

# Burst mode / HDR+
burst-mode-hold-steady = Hold steady...
burst-mode-frames = { $captured }/{ $total } frames
burst-mode-processing = Processing...
burst-mode-quality = Quality (FFT)
burst-mode-fast = Fast (Spatial)

# HDR+ dropdown options
hdr-plus-off = Off
hdr-plus-auto = Auto
hdr-plus-frames-4 = 4 frames
hdr-plus-frames-6 = 6 frames
hdr-plus-frames-8 = 8 frames
hdr-plus-frames-50 = 50 frames

# Photo settings
settings-photo = Photo
settings-photo-format = Output format
settings-photo-format-description = File format for saved photos. JPEG is compressed, PNG is lossless, DNG preserves raw data for editing.
settings-hdr-plus = HDR+ (experimental)
settings-hdr-plus-description = Multi-frame capture for improved low-light photos and dynamic range. Auto selects frame count based on scene brightness.
settings-burst-mode-quality = HDR+ algorithm
settings-burst-mode-quality-description = Quality uses FFT frequency domain merge for best results. Fast uses spatial merge for quicker processing.
settings-save-burst-raw = Save raw burst frames
settings-save-burst-raw-description = Save individual burst frames as DNG files alongside HDR+ photos. Useful for debugging or reprocessing.

# About page
about-support = Support & Feedback

# Insights
insights-title = Insights
insights-pipeline = Pipeline
insights-pipeline-full = GStreamer Pipeline
insights-pipeline-full-libcamera = Pipeline
insights-decoder-chain = Decoder Fallback Chain

insights-stream-combined = Preview + Capture Stream

insights-frame-latency = Frame Latency
insights-dropped-frames = Dropped Frames
insights-frame-size-decoded = Frame Size
insights-decode-time-gst = Buffer Processing
insights-copy-time = Frame Wrap Time
insights-gpu-upload-time = GPU Upload Time
insights-gpu-upload-bandwidth = GPU Upload Bandwidth

insights-format-source = Source
insights-format-resolution = Resolution
insights-format-framerate = Framerate
insights-format-native = Native Format
insights-format-gstreamer = GStreamer Output
insights-cpu-processing = CPU Processing
insights-cpu-decode-time = CPU Decode Time
insights-format-wgpu = GPU Processing

insights-selected = Selected
insights-available = Available
insights-unavailable = Unavailable

# Insights - Backend
insights-backend = Backend
insights-backend-type = Type
insights-pipeline-handler = Pipeline Handler
insights-libcamera-version = libcamera Version
insights-sensor-model = Sensor
insights-mjpeg-decoder = MJPEG Decoder

# Insights - Multi-stream
insights-multistream-single = Single-stream
insights-multistream-dual = Dual-stream
insights-multistream-source-shared = Preview & Capture
insights-multistream-source-separate = Preview / Capture
insights-stream-preview = Preview Stream
insights-stream-capture = Capture Stream
insights-stream-role = Role
insights-stream-resolution = Resolution
insights-stream-pixel-format = Pixel Format
insights-stream-frame-count = Frames

# Insights - Audio
insights-audio = Audio
insights-audio-recording = Recording
insights-audio-device = Device
insights-audio-node = Node
insights-audio-codec = Codec
insights-audio-channels = Channels
insights-audio-enabled = Enabled
insights-audio-disabled = Disabled
insights-audio-default = (Default)
insights-audio-mono = Mono
insights-audio-pipeline = Pipeline
insights-audio-format = Format
insights-audio-inputs = Input Channels
insights-audio-output-level = Output Level
insights-audio-not-recording = Not Recording

# Insights - Per-frame metadata
insights-metadata = Frame Metadata
insights-meta-exposure = Exposure
insights-meta-analogue-gain = Analogue Gain
insights-meta-digital-gain = Digital Gain
insights-meta-colour-temp = Colour Temp
insights-meta-sequence = Sequence
insights-meta-colour-gains = WB Gains (R, B)
insights-meta-black-level = Black Level
insights-meta-lens-position = Lens Position
insights-meta-lux = Illuminance
insights-meta-focus-fom = Focus FoM
insights-meta-na = N/A
