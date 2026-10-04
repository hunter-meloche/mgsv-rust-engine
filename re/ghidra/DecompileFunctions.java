// Decompile a list of functions to C for reference while porting.
//
// Input : text file of entry addresses (hex, one per line, '#' comments allowed)
//         default re/out/decompile_targets.txt
// Output: re/out/decomp/<address>.c (gitignored; derived from the retail binary,
//         used only as a local reference and never committed).
//@category MGSV

import java.io.File;
import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;

import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileOptions;
import ghidra.app.decompiler.DecompileResults;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.Function;

public class DecompileFunctions extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        File in = new File(args.length > 0 ? args[0] : "re/out/decompile_targets.txt");
        File dir = new File(args.length > 1 ? args[1] : "re/out/decomp");
        dir.mkdirs();

        DecompInterface di = new DecompInterface();
        di.setOptions(new DecompileOptions());
        di.openProgram(currentProgram);
        int ok = 0, fail = 0;
        for (String line : Files.readAllLines(in.toPath(), StandardCharsets.UTF_8)) {
            line = line.split("#")[0].trim();
            if (line.isEmpty() || monitor.isCancelled()) continue;
            Function f = getFunctionAt(toAddr(line));
            if (f == null) f = getFunctionContaining(toAddr(line));
            if (f == null) { fail++; continue; }
            DecompileResults r = di.decompileFunction(f, 120, monitor);
            if (!r.decompileCompleted()) { fail++; continue; }
            try (PrintWriter w = new PrintWriter(new File(dir, f.getEntryPoint() + ".c"), "UTF-8")) {
                w.println("// " + f.getName(true) + " @ " + f.getEntryPoint());
                w.print(r.getDecompiledFunction().getC());
            }
            ok++;
        }
        di.dispose();
        println("decompiled " + ok + ", failed " + fail + " -> " + dir.getAbsolutePath());
    }
}
