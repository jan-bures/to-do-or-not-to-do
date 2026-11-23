import org.gradle.api.DefaultTask
import org.gradle.api.file.ConfigurableFileCollection
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.provider.ListProperty
import org.gradle.api.provider.Property
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.InputFiles
import org.gradle.api.tasks.Internal
import org.gradle.api.tasks.OutputDirectory
import org.gradle.api.tasks.PathSensitive
import org.gradle.api.tasks.PathSensitivity
import org.gradle.api.tasks.TaskAction
import org.gradle.process.ExecOperations
import java.io.ByteArrayOutputStream
import java.io.File
import javax.inject.Inject

/** The Rust sources the core is built from, for up-to-date checks. */
fun rustInputs(repoRoot: File): List<Any> = listOf(
    File(repoRoot, "Cargo.toml"),
    File(repoRoot, "Cargo.lock"),
    File(repoRoot, "rust-toolchain.toml"),
    File(repoRoot, "crates/todo-core"),
    File(repoRoot, "crates/todo-client"),
    File(repoRoot, "crates/todo-design/src"),
    File(repoRoot, "crates/todo-ffi"),
    File(repoRoot, "assets/icons"),
)

abstract class CargoTask : DefaultTask() {
    @get:Inject
    abstract val exec: ExecOperations

    /** The root of the Cargo workspace (the repository). */
    @get:Internal
    abstract val workspace: DirectoryProperty

    @get:InputFiles
    @get:PathSensitive(PathSensitivity.RELATIVE)
    abstract val sources: ConfigurableFileCollection

    protected fun cargo(vararg args: String, env: Map<String, String> = emptyMap()) {
        exec.exec {
            workingDir = workspace.get().asFile
            environment(env)
            commandLine(listOf("cargo") + args)
        }
    }
}

/**
 * Builds `libtodo_ffi.so` for the given Android ABIs with cargo-ndk (release profile: the debug build
 * carries over 100 MB of debug info) into `outputDir/<abi>/`.
 */
abstract class CargoNdkTask : CargoTask() {
    @get:Input
    abstract val abis: ListProperty<String>

    @get:Input
    abstract val platform: Property<Int>

    @get:Input
    abstract val stripSymbols: Property<Boolean>

    @get:Internal
    abstract val gradleHome: DirectoryProperty

    @get:Internal
    abstract val sdkDir: DirectoryProperty

    @get:OutputDirectory
    abstract val outputDir: DirectoryProperty

    @TaskAction
    fun build() {
        val out = outputDir.get().asFile
        out.deleteRecursively()
        val targets = abis.get().flatMap { listOf("-t", it) }
        cargo(
            "ndk", *targets.toTypedArray(),
            "-P", platform.get().toString(),
            "-o", out.absolutePath,
            "build", "-p", "todo-ffi", "--release", "--locked",
            env = if (stripSymbols.get()) publicReleaseEnvironment() else emptyMap(),
        )
    }

    // Paths can survive in panic locations and other read-only data even after symbol stripping.
    private fun publicReleaseEnvironment(): Map<String, String> {
        val root = workspace.get().asFile
        val sysrootOutput = ByteArrayOutputStream()
        exec.exec {
            workingDir = root
            commandLine("rustc", "--print", "sysroot")
            standardOutput = sysrootOutput
        }
        val home = File(System.getProperty("user.home"))
        fun envDir(key: String, fallback: File) = System.getenv(key)?.takeIf(String::isNotBlank)?.let(::File) ?: fallback
        val paths = mutableListOf(
            home to "/build-user",
            root to "/src",
            envDir("CARGO_HOME", home.resolve(".cargo")) to "/cargo",
            envDir("RUSTUP_HOME", home.resolve(".rustup")) to "/rustc",
            File(sysrootOutput.toString(Charsets.UTF_8).trim()) to "/rustc",
            gradleHome.get().asFile to "/gradle",
            sdkDir.get().asFile to "/android-sdk",
        )
        for (variable in listOf("ANDROID_NDK_HOME", "ANDROID_NDK_ROOT", "ANDROID_HOME", "ANDROID_SDK_ROOT")) {
            System.getenv(variable)?.takeIf(String::isNotBlank)?.let {
                paths += File(it) to if ("NDK" in variable) "/android-ndk" else "/android-sdk"
            }
        }
        val remaps = paths.flatMap { (path, target) ->
            listOf(path.absolutePath, path.canonicalPath).flatMap { spelling ->
                listOf(spelling, spelling.replace('\\', '/')).map { it to target }
            }
        }.distinctBy { it.first }.sortedBy { it.first.length }.map { (from, to) -> "--remap-path-prefix=$from=$to" }
        // Use Cargo's encoded form so paths with spaces remain single arguments. A release uses the
        // declared flags only (and the target flags cargo-ndk supplies), never workstation overrides.
        return mapOf(
            "CARGO_PROFILE_RELEASE_STRIP" to "symbols",
            "CARGO_ENCODED_RUSTFLAGS" to remaps.joinToString("\u001f"),
        )
    }
}

/**
 * Builds the core for this machine and generates the Kotlin bindings from it. The host library is also
 * what the JVM unit and screenshot tests load through JNA (`jna.library.path` = [libraryDir]).
 */
abstract class UniffiBindingsTask : CargoTask() {
    @get:OutputDirectory
    abstract val outputDir: DirectoryProperty

    /** Where cargo puts the host library (`target/debug`). */
    @get:Internal
    abstract val libraryDir: DirectoryProperty

    @TaskAction
    fun generate() {
        cargo("build", "-p", "todo-ffi", "--features", "bindgen", "--locked")
        val os = System.getProperty("os.name").lowercase()
        val (lib, bindgen) = when {
            os.contains("windows") -> "todo_ffi.dll" to "uniffi-bindgen.exe"
            os.contains("mac") -> "libtodo_ffi.dylib" to "uniffi-bindgen"
            else -> "libtodo_ffi.so" to "uniffi-bindgen"
        }
        val dir = libraryDir.get().asFile
        val out = outputDir.get().asFile
        out.deleteRecursively()
        exec.exec {
            workingDir = workspace.get().asFile
            commandLine(
                File(dir, bindgen).absolutePath, "generate",
                "--library", File(dir, lib).absolutePath,
                "--language", "kotlin",
                "--out-dir", out.absolutePath,
                "--no-format",
            )
        }
    }
}
