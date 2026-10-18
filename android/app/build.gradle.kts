import groovy.json.JsonSlurper

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.kotlin.serialization)
}

val repoDir: File = rootDir.parentFile
val secretsDir = File(repoDir, "secrets")
val brandAssetsDir = File(repoDir, "assets")

// Firebase client config lives in the gitignored secrets/ directory. It is copied next to this
// script so the google-services plugin can find it; without it the app builds with push disabled.
val googleServicesTarget = file("google-services.json")
File(secretsDir, "google-services.json").takeIf { it.isFile }?.let { source ->
    if (!googleServicesTarget.isFile || googleServicesTarget.readText() != source.readText()) {
        source.copyTo(googleServicesTarget, overwrite = true)
    }
}
if (googleServicesTarget.isFile) {
    apply(plugin = "com.google.gms.google-services")
} else {
    logger.warn("Ring: secrets/google-services.json not found; building without FCM push.")
}

// Release signing reuses the existing Ring key so installed APKs update in place. Values come from
// the environment (CI) or from the gitignored secrets/ directory, never from tracked files.
data class ReleaseKey(val store: File, val alias: String, val storePassword: String, val keyPassword: String)

val releaseKey: ReleaseKey? = run {
    val envStore = System.getenv("RING_ANDROID_KEYSTORE_PATH")
    if (envStore != null) {
        return@run ReleaseKey(
            store = file(envStore),
            alias = System.getenv("RING_ANDROID_KEY_ALIAS") ?: "ring",
            storePassword = requireNotNull(System.getenv("RING_ANDROID_KEYSTORE_PASSWORD")) { "RING_ANDROID_KEYSTORE_PASSWORD is not set" },
            keyPassword = requireNotNull(System.getenv("RING_ANDROID_KEY_PASSWORD")) { "RING_ANDROID_KEY_PASSWORD is not set" },
        )
    }
    val descriptor = File(secretsDir, "silicon-ring-android-signing.private.json")
    if (!descriptor.isFile) return@run null
    val fields = JsonSlurper().parse(descriptor) as Map<*, *>
    // The path recorded in the descriptor may be stale; the keystore normally sits beside it.
    val store = File(secretsDir, "silicon-ring-android-release.p12").takeIf { it.isFile }
        ?: File(fields["keystore"] as String)
    ReleaseKey(
        store = store,
        alias = fields["alias"] as String,
        storePassword = fields["store_password"] as String,
        keyPassword = fields["key_password"] as String,
    )
}

android {
    namespace = "com.teamofsilicons.ring"
    compileSdk = 36

    defaultConfig {
        applicationId = "com.teamofsilicons.ring"
        minSdk = 26
        targetSdk = 36
        versionCode = 1_000_003
        versionName = "1.0.0"
        buildConfigField("String", "ACCOUNTS_URL", "\"https://accounts.teamofsilicons.com\"")
        buildConfigField("String", "SOURCE_URL", "\"https://github.com/teamofsilicons/silicon-ring\"")
        ndk {
            abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64")
        }
    }

    signingConfigs {
        if (releaseKey != null) {
            create("release") {
                storeFile = releaseKey.store
                storeType = "pkcs12"
                storePassword = releaseKey.storePassword
                keyAlias = releaseKey.alias
                keyPassword = releaseKey.keyPassword
            }
        }
    }

    buildTypes {
        debug {
            buildConfigField("String", "DEFAULT_API_URL", "\"http://192.168.29.75:8765\"")
        }
        release {
            buildConfigField("String", "DEFAULT_API_URL", "\"https://backend.ring.teamofsilicons.com\"")
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            if (releaseKey != null) {
                signingConfig = signingConfigs.getByName("release")
            } else {
                logger.warn("Ring: no release signing key found; the release APK will be unsigned.")
            }
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    packaging {
        resources.excludes += listOf("META-INF/*.version", "META-INF/LICENSE*", "META-INF/NOTICE*", "DebugProbesKt.bin")
    }
}

// Brand fonts and sounds are shared by every Ring client under ../assets. They are copied into a
// generated resource directory (renamed to valid resource names) instead of being duplicated here.
abstract class SyncBrandResources : DefaultTask() {
    @get:InputDirectory
    @get:PathSensitive(PathSensitivity.RELATIVE)
    abstract val assetsDir: DirectoryProperty

    @get:OutputDirectory
    abstract val outputDir: DirectoryProperty

    @get:Inject
    abstract val fs: FileSystemOperations

    @TaskAction
    fun sync() {
        val fonts = mapOf(
            "bdo-grotesk/BDOGrotesk-Regular.ttf" to "bdo_grotesk_regular.ttf",
            "bdo-grotesk/BDOGrotesk-Medium.ttf" to "bdo_grotesk_medium.ttf",
            "bdo-grotesk/BDOGrotesk-Bold.ttf" to "bdo_grotesk_bold.ttf",
            "hedvig-letters-serif/HedvigLettersSerif-Regular.ttf" to "hedvig_letters_serif.ttf",
            "geist-pixel/GeistPixel-Square.ttf" to "geist_pixel_square.ttf",
        )
        val root = assetsDir.get().asFile
        fs.sync {
            into(outputDir)
            fonts.forEach { (source, target) ->
                from(File(root, "fonts/$source")) {
                    into("font")
                    rename { target }
                }
            }
            from(File(root, "sounds")) {
                include("*.wav")
                into("raw")
            }
        }
    }
}

val syncBrandResources = tasks.register<SyncBrandResources>("syncBrandResources") {
    assetsDir.set(brandAssetsDir)
    outputDir.set(layout.buildDirectory.dir("generated/ring/res"))
}

androidComponents {
    onVariants { variant ->
        variant.sources.res?.addGeneratedSourceDirectory(syncBrandResources, SyncBrandResources::outputDir)
    }
}

dependencies {
    implementation(platform(libs.compose.bom))
    implementation(libs.compose.ui)
    implementation(libs.compose.foundation)
    implementation(libs.compose.material3)
    implementation(libs.compose.ui.tooling.preview)
    debugImplementation(libs.compose.ui.tooling)
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.core.telecom)
    implementation(libs.androidx.browser)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.androidx.lifecycle.process)
    implementation(libs.androidx.lifecycle.service)
    implementation(platform(libs.firebase.bom))
    implementation(libs.firebase.messaging)
    implementation(libs.livekit.android)
    implementation(libs.okhttp)
    implementation(libs.kotlinx.serialization.json)
    implementation(libs.kotlinx.coroutines.android)
    implementation(libs.kotlinx.coroutines.play.services)
    implementation(libs.coil.compose)
    implementation(libs.coil.network.okhttp)
}
