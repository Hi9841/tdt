# TDT for Android

Talk, don't type, on Android. A small bubble slides out beside your own keyboard in any text field. Tap it to talk and tap again to stop, or hold it while you talk. The words go in at the cursor.

Speech stays on the phone. TDT runs Parakeet Redux, the same model as [TDT desktop](https://github.com/Hi9841/tdt), on the same engine, [Trit](https://github.com/Hi9841/trit). On phones with 6 GB of memory or more, you can pick [Voz](https://desertant.com/models/voz/) by Desert Ant Labs instead (25 languages, 1.25 GB). The network is used only to download the model you pick.

## Build

Needs the Android SDK with NDK 27.1.12297006 and CMake 4.1.2, JDK 21, and Rust (stable) with the Android targets:

```sh
rustup target add aarch64-linux-android x86_64-linux-android
./gradlew :app:assembleDebug
```

The speech code has two parts:

- `third_party/trit` is the Trit engine: Trit 0.15.0, an export of the `v0.15.0` tag, with no changes. It has the arm64 kernels (NEON; `sdot` where the CPU has the Armv8.2 dot product; `smmla` for the int8 layers where it has the Armv8.6 int8 matrix multiply), and its worker team follows the cores that Android gives the app.
- `engine/` is a small Rust library (`tdt-speech`) on top of Trit. It is the take path of TDT desktop: the model reads a take while you talk, so the text is ready soon after you stop. The model loads when the bubble service starts and stays in memory. Trit maps the weights from a file next to the model (`.trit/`, about the size of the model again), so Android can drop those pages when memory is short.

CMake builds `engine/` with cargo for each ABI and links it into `libtdt_engine.so`. To build with another Trit version, replace `third_party/trit` with an export of that version.

### Release

Release builds are signed with the key named in `keystore.properties` (gitignored):

```properties
storeFile=C:/path/to/tdt-release.jks
storePassword=...
keyAlias=tdt
keyPassword=...
```

```sh
./gradlew :app:bundleRelease :app:assembleRelease
```

Release APKs contain only arm64-v8a. x86_64 emulators with Android 11 or later run them through ARM translation, slowly; use debug builds there. A debug build and a release build have different keys, so one does not install over the other: uninstall first.

`./gradlew :app:assembleSideload` builds the same app without the bubble (see Install).

## Install

TDT needs an arm64 phone with Android 10 or later. Use the signed release APK (`dist/TDT-X.Y.Z-arm64.apk`). It installs over an earlier TDT release and keeps the model.

The surest way is USB:

1. On the phone, open **Settings > About phone** and tap **Build number** 7 times. Then open **Settings > System > Developer options** and turn on **USB debugging**.
2. Connect the phone to the computer with a USB cable and accept the prompt on the phone.
3. On the computer: `adb install -r dist/TDT-0.6.1-arm64.apk`

Wireless debugging works the same without a cable (Android 11+): in **Developer options > Wireless debugging**, tap **Pair device with pairing code**, run `adb pair <ip>:<pairing port> <code>`, then `adb devices` (adb finds the connect port by itself after pairing) and `adb install -r`.

Without a cable: copy the APK to the phone, open it in Files, and allow the install when Android asks. If Play Protect shows a warning, tap **More details > Install anyway**. In some countries Play Protect blocks apps that install from a browser or a file manager and ask for Accessibility. If it blocks TDT, install with USB, or install `TDT-X.Y.Z-arm64-sideload.apk`: it has no bubble, and you dictate in the app and copy the text.

## Use

1. Open TDT, pick a speech model, and download it (179 MB for Parakeet Redux).
2. Tap **Allow microphone**.
3. Tap **Open Accessibility** and turn on **TDT bubble**. If Android says the setting is restricted, open **App info**, tap the menu, choose **Allow restricted settings**, and try again.
4. Tap any text field. The bubble appears beside the keyboard.

Drag the bubble to either edge. Drag it to the bottom of the screen to hide it for 10 minutes.

With Parakeet Redux, add names and terms to the **Dictionary**, one per line, spelled as you want them written. TDT ships no terms of its own.

## Test

```sh
./gradlew :app:testDebugUnitTest
# Instrumented tests need the model in the app; install the APKs yourself,
# because connectedAndroidTest uninstalls the app and its model afterwards.
adb shell am instrument -w io.github.hi9841.tdt.test/androidx.test.runner.AndroidJUnitRunner
# On a phone with the release build: sign the test with the release key, so it runs in that app.
./gradlew -Ptdt.testBuildType=release :app:assembleReleaseAndroidTest
# Take times and insert facts (never the text) from the app:
adb logcat -s TDT:I
```

To run WAV files through the phone's speech path on a computer, and compare the text with TDT desktop:

```sh
cd engine
cargo run --release --example takes -- --model <model folder> --dir <folder of 16 kHz WAV files>
```

## Docs

- `docs/MAP.md`: decisions, risks and open items.
- `docs/adr/0001-run-redux-with-parakeetogo.md`: why not Photon on Android.
- `docs/adr/0002-run-redux-with-trit-and-offer-voz.md`: why Trit replaced parakeetogo, and the cost of Voz on Android.
- `docs/adr/0003-run-the-take-path-of-tdt-desktop.md`: why the phone reads a take while you talk, with the code of TDT desktop.
- `GLOSSARY.md`: project terms.
- `THIRD_PARTY_NOTICES.md`: model and library licenses.
