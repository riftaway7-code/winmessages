# winmessages

## STILL EARLY DEVELOPMENT :)

- Open issues if you find any. Pull requests if you have fixes. Please star!!

native imessage on windows. no mac, no iphone, no relay server. the pc itself pretends to BE a mac and logs into a real apple id.

yes aiyan it's a tauri app on the outside (react front in `app/src`, rust back in `app/src-tauri`). the whole point is underneath that.

## what actually does the work
- `app/src-tauri/src/engine.rs`: the real file. holds a fake mac hardware identity (`baked()`: a mac serial + the encrypted "gestalt" stamp apple checks), logs into the apple id, registers the pc as a mac, sends/receives real imessages.
- `vendor/rustpush`: the apple push/IDS/imessage protocol (openbubbles fork). engine.rs drives this.
- the trick: apple verifies a hardware stamp server-side. we generate a valid one in pure software (the base64 `_enc` values in `baked()`), so it works with zero apple hardware.

## the current stage (this is "what's wrong")
sign-in reaches apple fine but apple returns **"iMessage Activation Failed" (symptom 23367)**. that is NOT a code bug. it's apple rate-limiting new imessage activations from this pc/account because we tried too many times today. it's a cooldown. every sign-in press resets the timer. fix = leave it ~24h untouched, then ONE clean try.

proof the code is right: it already delivered a real imessage to a real iphone earlier today on a fresh fake-mac identity.

## before you sign in
- the apple account needs **Contact Key Verification off**. with it on, the password and 2fa are accepted and then registration fails with `IDSError(6001)`. every openbubbles-based client has the same limit. if you want it on for your main account, sign in with a separate one.
- everyone using this build shares one baked-in hardware identity (`baked()`), so apple can block it.
- apple sends a "new device" alert when you sign in.
- the login keys are plain files in the data dir (`keystore.plist`), not encrypted.

## run it
```
cd app
npm install
npm run tauri dev
```
needs a vite dev server (npm run dev auto-starts via tauri) + a rust build toolchain: msvc, protoc, perl, libclang, cmake. `app/src-tauri/Cargo.toml` path-depends on `../../vendor/rustpush`, so keep the folder layout as-is.

## linux
same commands as above. only tried on arch linux (omarchy, hyprland, wayland, intel graphics).

arch packages: `webkit2gtk-4.1 gtk3 librsvg openssl protobuf clang cmake perl libatomic nodejs npm rust`. other distros: the packages from tauri's linux prerequisites page, plus protoc, perl, libclang, cmake and libatomic. untested outside arch.

`npm run tauri build` makes a `.deb` and an `.rpm` in `app/src-tauri/target/release/bundle`. a bundle built on a new distro needs that distro's glibc: the one built on arch needs glibc 2.39. it installed on debian 12 and ubuntu 22.04 but won't start there (glibc 2.36 and 2.35). on fedora 42 the rpm installed and every library resolved (the app was not run there). for an older distro build on it, or on the oldest one you want to support.

on linux the data dir is `~/.local/share/winmessages`.
