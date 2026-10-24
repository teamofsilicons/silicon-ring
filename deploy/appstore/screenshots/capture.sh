#!/bin/sh
# Captures the App Store screenshots from Ring's demo mode (-RingDemo, Debug builds only), which
# fills the app with made-up people and calls and needs no server or sign-in.
#
#   deploy/appstore/screenshots/capture.sh <iphone-6.9-udid> <ipad-13-udid>
#
# Use an iPhone 17 Pro Max simulator (1320 x 2868) and an iPad Pro 13-inch simulator (2064 x 2752).
# Each is set to US English with a 12-hour clock (the iPad to full-screen apps, without window
# controls), restarted, and given a 9:41 status bar. Screens are light; the in-call screen is also
# captured dark. flatten.swift then drops the alpha channel and the Dynamic Island the iPhone
# simulator sometimes draws. Build the Debug app for the simulator first:
#
#   xcodebuild -project apple/Ring.xcodeproj -scheme Ring-iOS -configuration Debug \
#     -destination 'generic/platform=iOS Simulator' -derivedDataPath target/wf-shots-dd build
set -eu

cd "$(dirname "$0")/../../.."
app=${RING_APP:-target/wf-shots-dd/Build/Products/Debug-iphonesimulator/Ring.app}
bundle=com.teamofsilicons.ring
out=deploy/appstore/screenshots
[ $# -eq 2 ] || { echo "usage: $0 <iphone-udid> <ipad-udid>" >&2; exit 2; }
[ -d "$app" ] || { echo "no app at $app: build the Debug app for the simulator first" >&2; exit 1; }

prepare() { # udid folder
    xcrun simctl boot "$1" 2>/dev/null || true
    xcrun simctl bootstatus "$1" -b >/dev/null
    xcrun simctl spawn "$1" defaults write "Apple Global Domain" AppleLanguages -array en-US
    xcrun simctl spawn "$1" defaults write "Apple Global Domain" AppleLocale -string en_US
    xcrun simctl spawn "$1" defaults write "Apple Global Domain" AppleICUForce24HourTime -bool false
    if [ "$2" = ipad-13 ]; then
        xcrun simctl spawn "$1" defaults write com.apple.springboard SBMedusaMultitaskingEnabled -bool false
    fi
    xcrun simctl shutdown "$1"
    xcrun simctl boot "$1"
    xcrun simctl bootstatus "$1" -b >/dev/null
    xcrun simctl status_bar "$1" override --time 9:41 --dataNetwork wifi --wifiMode active --wifiBars 3 \
        --cellularMode active --cellularBars 4 --operatorName "" --batteryState discharging --batteryLevel 100
    xcrun simctl install "$1" "$app"
    # The first launch after a restart is slow; get it out of the way.
    xcrun simctl launch "$1" "$bundle" -RingDemo recents >/dev/null
    sleep 15
    mkdir -p "$out/$2"
}

shoot() { # udid folder number name scene appearance
    xcrun simctl ui "$1" appearance "$6"
    xcrun simctl terminate "$1" "$bundle" >/dev/null 2>&1 || true
    xcrun simctl launch "$1" "$bundle" -RingDemo "$5" >/dev/null
    # Launch, then the dialer and the voicemail open on top once the home screen is up.
    sleep 18
    xcrun simctl io "$1" screenshot --type=png "$out/$2/$3-$4.png" >/dev/null 2>&1
    echo "$out/$2/$3-$4.png"
}

for pair in "$1 iphone-6.9" "$2 ipad-13"; do
    set -- $pair
    prepare "$1" "$2"
    shoot "$1" "$2" 01 call call light
    shoot "$1" "$2" 02 incoming incoming light
    shoot "$1" "$2" 03 recents recents light
    shoot "$1" "$2" 04 voicemail voicemail light
    shoot "$1" "$2" 05 new-call dialer light
    shoot "$1" "$2" 06 call-dark call dark
    xcrun simctl terminate "$1" "$bundle" >/dev/null 2>&1 || true
    xcrun simctl ui "$1" appearance light
done

xcrun swift "$out/flatten.swift" "$out"/iphone-6.9/*.png "$out"/ipad-13/*.png
