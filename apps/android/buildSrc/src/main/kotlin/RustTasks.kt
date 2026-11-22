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

    protected fun cargo(vararg args: String) {
        exec.exec {
            workingDir = workspace.get().asFile
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
