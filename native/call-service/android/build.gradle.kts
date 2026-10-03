import org.jetbrains.kotlin.gradle.dsl.JvmTarget
plugins { id("com.android.library"); id("org.jetbrains.kotlin.android") }
android {
    namespace = "com.teamofsilicons.callservice"
    compileSdk = 36
    defaultConfig { minSdk = 26; consumerProguardFiles("consumer-rules.pro") }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_1_8; targetCompatibility = JavaVersion.VERSION_1_8 }
}
kotlin { compilerOptions { jvmTarget = JvmTarget.JVM_1_8 } }
dependencies {
    implementation("androidx.core:core-ktx:1.13.1")
    implementation("com.squareup.okhttp3:okhttp:4.12.0")
    implementation("com.google.firebase:firebase-messaging:24.1.0")
    implementation(project(":tauri-android"))
}
