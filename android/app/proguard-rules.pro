# kotlinx.serialization: keep generated serializers for the wire models.
-keepattributes *Annotation*, InnerClasses, Signature, EnclosingMethod
-keepclassmembers @kotlinx.serialization.Serializable class com.teamofsilicons.ring.** {
    *** Companion;
    *** INSTANCE;
    kotlinx.serialization.KSerializer serializer(...);
}
-keepclasseswithmembers class com.teamofsilicons.ring.** {
    kotlinx.serialization.KSerializer serializer(...);
}
-if @kotlinx.serialization.Serializable class com.teamofsilicons.ring.**
-keepclassmembers class <1>$Companion {
    kotlinx.serialization.KSerializer serializer(...);
}

# LiveKit and its bundled WebRTC are reached through JNI and reflection.
-keep class livekit.org.webrtc.** { *; }
-keep class io.livekit.android.** { *; }
-keep class livekit.** { *; }
-dontwarn org.slf4j.**
-dontwarn com.google.errorprone.annotations.**
-dontwarn javax.annotation.**
