import com.android.build.api.variant.ApplicationAndroidComponentsExtension

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.paparazzi)
}

// The repository root: the Cargo workspace, the design tokens, fonts and icons.
val repoRoot: File = rootDir.resolve("../..").canonicalFile

// ABIs to build the Rust core for: phones (arm64-v8a) and the emulator (x86_64). `-Ptodo.abis=x86_64`
// builds one, for a faster loop on the emulator.
val androidAbis: List<String> = (findProperty("todo.abis") as String? ?: "arm64-v8a,x86_64").split(',').map(String::trim)
val minSdkVersion = 29

android {
    namespace = "app.todoornot"
    // Compose 1.13 needs the 37.1 APIs or later to compile against.
    compileSdk {
        version = release(37) { minorApiLevel = 2 }
    }

    defaultConfig {
        applicationId = "app.todoornot.android"
        minSdk = minSdkVersion
        targetSdk = 37
        versionCode = 1
        versionName = "0.1.0"
        // Plain http to a loopback server (the emulator workflow). Off unless a build type turns it on:
        // only `debug` does, so a release build, even one made debuggable, talks HTTPS only.
        buildConfigField("boolean", "ALLOW_LOOPBACK_HTTP", "false")
    }

    buildTypes {
        debug {
            buildConfigField("boolean", "ALLOW_LOOPBACK_HTTP", "true")
        }
        release {
            isMinifyEnabled = false
            // Sideloaded only. No signing secrets live in this repository: sign release builds locally.
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_21
        targetCompatibility = JavaVersion.VERSION_21
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    testOptions {
        unitTests.isReturnDefaultValues = true
        unitTests.isIncludeAndroidResources = true
    }
}

kotlin {
    jvmToolchain(21)
}

// --- Generated sources -----------------------------------------------------------------------------

val rustSources = files(rustInputs(repoRoot))

val cargoNdk = tasks.register<CargoNdkTask>("cargoNdkBuild") {
    description = "Builds the Rust core (crates/todo-ffi) for Android with cargo-ndk."
    workspace.set(repoRoot)
    sources.from(rustSources)
    abis.set(androidAbis)
    platform.set(minSdkVersion)
    outputDir.set(layout.buildDirectory.dir("generated/rust/jniLibs"))
}

val hostLibDir = repoRoot.resolve("target/debug")

val uniffiBindings = tasks.register<UniffiBindingsTask>("uniffiBindings") {
    description = "Builds the Rust core for this machine and generates its Kotlin bindings."
    workspace.set(repoRoot)
    sources.from(rustSources)
    libraryDir.set(hostLibDir)
    outputDir.set(layout.buildDirectory.dir("generated/uniffi/kotlin"))
}

val designTokens = tasks.register<GenerateDesignTokens>("generateDesignTokens") {
    description = "Generates Tokens.kt from crates/todo-design/generated/tokens.json."
    tokens.set(repoRoot.resolve("crates/todo-design/generated/tokens.json"))
    outputDir.set(layout.buildDirectory.dir("generated/design/kotlin"))
}

val designResources = tasks.register<GenerateDesignResources>("generateDesignResources") {
    description = "Generates the window background colours from tokens.json."
    tokens.set(repoRoot.resolve("crates/todo-design/generated/tokens.json"))
    outputDir.set(layout.buildDirectory.dir("generated/design/res"))
}

val launcherIcon = tasks.register<GenerateLauncherIcon>("generateLauncherIcon") {
    description = "Generates the launcher icon's adaptive layers from assets/brand."
    brand.set(repoRoot.resolve("assets/brand"))
    outputDir.set(layout.buildDirectory.dir("generated/brand/res"))
}

val icons = tasks.register<GenerateIcons>("generateIcons") {
    description = "Generates IconPaths.kt from assets/icons."
    this.icons.set(repoRoot.resolve("assets/icons"))
    outputDir.set(layout.buildDirectory.dir("generated/icons/kotlin"))
}

val fonts = tasks.register<PrepareFonts>("prepareFonts") {
    description = "Copies assets/fonts into a generated res/font."
    this.fonts.set(repoRoot.resolve("assets/fonts"))
    outputDir.set(layout.buildDirectory.dir("generated/fonts/res"))
}

extensions.getByType<ApplicationAndroidComponentsExtension>().onVariants { variant ->
    variant.sources.kotlin?.let {
        it.addGeneratedSourceDirectory(uniffiBindings, UniffiBindingsTask::outputDir)
        it.addGeneratedSourceDirectory(designTokens, GenerateDesignTokens::outputDir)
        it.addGeneratedSourceDirectory(icons, GenerateIcons::outputDir)
    }
    variant.sources.res?.let {
        it.addGeneratedSourceDirectory(fonts, PrepareFonts::outputDir)
        it.addGeneratedSourceDirectory(designResources, GenerateDesignResources::outputDir)
        it.addGeneratedSourceDirectory(launcherIcon, GenerateLauncherIcon::outputDir)
    }
    variant.sources.jniLibs?.addGeneratedSourceDirectory(cargoNdk, CargoNdkTask::outputDir)

    val name = variant.name.replaceFirstChar(Char::uppercase)
    val policy = tasks.register<VerifyCleartextPolicy>("verify${name}CleartextPolicy") {
        description = "Checks that only the debug build type allows plain http to loopback."
        buildType.set(variant.buildType ?: "")
        allowLoopbackHttp.set(
            variant.buildConfigFields?.getting("ALLOW_LOOPBACK_HTTP")?.map { it.value.toString() } ?: provider { "missing" },
        )
    }
    tasks.matching { it.name == "pre${name}Build" }.configureEach { dependsOn(policy) }
}

// The JVM tests (unit and Paparazzi screenshots) load the host build of the core through JNA.
tasks.withType<Test>().configureEach {
    // Layoutlib (Paparazzi) and Robolectric load incompatible native Android runtimes.
    // Keep them in separate worker JVMs, independent of JUnit’s class execution order.
    forkEvery = 1
    dependsOn(uniffiBindings)
    systemProperty("jna.library.path", hostLibDir.absolutePath)
    // The checked-in tokens.json, for the test that compares it with the generated Kotlin.
    systemProperty("todo.tokens", repoRoot.resolve("crates/todo-design/generated/tokens.json").absolutePath)
    // The brand sources, for the test that checks the icon's key art against the safe zone.
    systemProperty("todo.brand", repoRoot.resolve("assets/brand").absolutePath)
}

// --- rustls-platform-verifier: its Kotlin half must match the Rust crate exactly -----------------

abstract class RustlsVerifierVersion : ValueSource<String, RustlsVerifierVersion.Params> {
    interface Params : ValueSourceParameters {
        val lockFile: RegularFileProperty
    }

    override fun obtain(): String {
        val lines = parameters.lockFile.get().asFile.readLines()
        val at = lines.indexOfFirst { it.trim() == "name = \"rustls-platform-verifier-android\"" }
        val version = lines.drop(at + 1).firstOrNull { it.trimStart().startsWith("version = ") }
            ?.substringAfter('"')?.substringBefore('"')
        return version?.takeIf { at >= 0 && it.isNotEmpty() }
            ?: error("rustls-platform-verifier-android not found in Cargo.lock")
    }
}

val rustlsVerifierVersion = providers.of(RustlsVerifierVersion::class.java) {
    parameters.lockFile.set(repoRoot.resolve("Cargo.lock"))
}

configurations.configureEach {
    resolutionStrategy.eachDependency {
        if (requested.group == "org.rustls" && requested.name == "rustls-platform-verifier") {
            useVersion(rustlsVerifierVersion.get())
            because("the Kotlin component must match rustls-platform-verifier-android in Cargo.lock")
        }
    }
}

dependencies {
    implementation(platform(libs.compose.bom))
    implementation(libs.compose.ui)
    implementation(libs.compose.ui.graphics)
    implementation(libs.compose.foundation)
    implementation(libs.compose.animation)
    implementation(libs.compose.material3)
    implementation(libs.compose.ui.tooling.preview)
    implementation(libs.activity.compose)
    implementation("com.journeyapps:zxing-android-embedded:4.3.0")
    implementation(libs.lifecycle.runtime.compose)
    implementation(libs.lifecycle.viewmodel.compose)
    implementation(libs.lifecycle.process)
    implementation(libs.adaptive)
    implementation(libs.glance.appwidget)
    implementation(libs.work.runtime)
    implementation(libs.firebase.messaging)
    implementation(libs.kotlinx.coroutines.android)
    implementation(libs.kotlinx.coroutines.playservices)
    implementation("${libs.jna.get().module}:${libs.versions.jna.get()}@aar")
    implementation(libs.rustls.platform.verifier)
    debugImplementation(libs.compose.ui.tooling)
    debugImplementation(libs.compose.ui.test.manifest)

    testImplementation(libs.compose.ui.test.junit4)
    testImplementation(libs.junit)
    testImplementation(libs.robolectric)
    testImplementation(libs.jna)
    testImplementation(libs.kotlinx.coroutines.test)
    testImplementation(libs.json)
}
