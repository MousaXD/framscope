import java.util.Properties
import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

val versionPropertiesFile = rootProject.file("../version.properties")
val frameScopeVersion = Properties().apply {
    versionPropertiesFile.inputStream().use(::load)
}
val frameScopeVersionName = frameScopeVersion.getProperty("versionName")
    ?.takeIf { value -> Regex("[0-9]+\\.[0-9]+\\.[0-9]+").matches(value) }
    ?: error("version.properties must define semantic versionName MAJOR.MINOR.PATCH")
val frameScopeVersionCode = frameScopeVersion.getProperty("versionCode")
    ?.toIntOrNull()
    ?.takeIf { value -> value > 0 }
    ?: error("version.properties must define a positive integer versionCode")

val releaseStorePath = providers.environmentVariable("FRAMESCOPE_SIGNING_STORE_FILE").orNull
val releaseStorePassword = providers.environmentVariable("FRAMESCOPE_SIGNING_STORE_PASSWORD").orNull
val releaseKeyAlias = providers.environmentVariable("FRAMESCOPE_SIGNING_KEY_ALIAS").orNull
val releaseKeyPassword = providers.environmentVariable("FRAMESCOPE_SIGNING_KEY_PASSWORD").orNull
val releaseSigningValues = listOf(
    releaseStorePath,
    releaseStorePassword,
    releaseKeyAlias,
    releaseKeyPassword,
)
val releaseSigningConfigured = releaseSigningValues.all { value -> !value.isNullOrBlank() }
check(releaseSigningValues.none { value -> !value.isNullOrBlank() } || releaseSigningConfigured) {
    "Release signing must provide store file, store password, key alias, and key password together."
}

android {
    namespace = "com.framescope.app"
    compileSdk = 36
    ndkVersion = "27.3.13750724"

    signingConfigs {
        if (releaseSigningConfigured) {
            create("release") {
                storeFile = file(requireNotNull(releaseStorePath))
                storePassword = releaseStorePassword
                keyAlias = releaseKeyAlias
                keyPassword = releaseKeyPassword
            }
        }
    }

    defaultConfig {
        applicationId = "com.framescope.app"
        minSdk = 26
        targetSdk = 36
        versionCode = frameScopeVersionCode
        versionName = frameScopeVersionName
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"

        ndk {
            abiFilters += setOf("arm64-v8a")
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            if (releaseSigningConfigured) {
                signingConfig = signingConfigs.getByName("release")
            }
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
        }
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    sourceSets {
        getByName("main") {
            jniLibs.srcDir(layout.buildDirectory.dir("generated/jniLibs"))
        }
    }

    testOptions {
        unitTests.all {
            it.useJUnit()
        }
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_17)
    }
}

val rustJniOutput = layout.buildDirectory.dir("generated/jniLibs")
val ffmpegRoot = providers.environmentVariable("FRAMESCOPE_FFMPEG_ROOT")
    .orElse(rootProject.file("../.native/ffmpeg/arm64-v8a").absolutePath)

val prepareFfmpegArm64 by tasks.registering(Exec::class) {
    group = "build"
    description = "Prepare the pinned FFmpeg Android arm64-v8a static prefix."
    workingDir = rootProject.file("..")
    inputs.file(rootProject.file("../scripts/build-ffmpeg-android.sh"))
    inputs.file(rootProject.file("../scripts/verify-ffmpeg-android.sh"))
    environment("FRAMESCOPE_FFMPEG_ROOT", ffmpegRoot.get())
    commandLine("bash", rootProject.file("../scripts/build-ffmpeg-android.sh").absolutePath)
}

val buildRustArm64 by tasks.registering(Exec::class) {
    group = "build"
    description = "Build the FFmpeg-linked Rust JNI library for arm64-v8a with cargo-ndk."
    dependsOn(prepareFfmpegArm64)
    workingDir = rootProject.file("../rust")
    inputs.file(rootProject.file("../rust/Cargo.toml"))
    inputs.dir(rootProject.file("../rust/crates"))
    inputs.file(rootProject.file("../scripts/build-rust.sh"))
    inputs.file(rootProject.file("../scripts/build-ffmpeg-android.sh"))
    inputs.file(rootProject.file("../scripts/verify-ffmpeg-android.sh"))
    inputs.file(versionPropertiesFile)
    outputs.dir(rustJniOutput)
    environment("CARGO_TARGET_DIR", rootProject.file("../rust/target").absolutePath)
    environment("FRAMESCOPE_FFMPEG_ROOT", ffmpegRoot.get())
    commandLine(
        "cargo",
        "ndk",
        "-t",
        "arm64-v8a",
        "-o",
        rustJniOutput.get().asFile.absolutePath,
        "build",
        "-p",
        "framescope-ffi",
        "--release",
    )
}

tasks.named("preBuild").configure {
    dependsOn(buildRustArm64)
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2026.06.00")
    implementation(composeBom)
    androidTestImplementation(composeBom)

    implementation("androidx.activity:activity-compose:1.13.0")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.10.0")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.10.0")
    implementation("androidx.lifecycle:lifecycle-viewmodel-ktx:2.10.0")
    implementation("androidx.navigation:navigation-compose:2.9.5")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.compose.foundation:foundation")
    implementation("androidx.compose.material:material-icons-core")
    implementation("androidx.compose.material3:material3")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.2")

    debugImplementation("androidx.compose.ui:ui-tooling")
    debugImplementation("androidx.compose.ui:ui-test-manifest")
    androidTestImplementation("androidx.compose.ui:ui-test-junit4")

    testImplementation("junit:junit:4.13.2")
    testImplementation("org.json:json:20260719")
    testImplementation("org.jetbrains.kotlinx:kotlinx-coroutines-test:1.10.2")
}
