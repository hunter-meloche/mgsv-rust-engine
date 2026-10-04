// Apply function names (and signature comments) from a CSV:
//   address(hex),name,comment
// Existing user-defined names are kept; default FUN_ names are replaced.
// Run without -readOnly so the project is saved.
//@category MGSV

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;

import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.CodeUnit;
import ghidra.program.model.listing.Function;
import ghidra.program.model.symbol.SourceType;

public class ApplyNames extends GhidraScript {
    @Override
    public void run() throws Exception {
        File in = new File(getScriptArgs().length > 0 ? getScriptArgs()[0] : "re/out/ihhook_names.csv");
        int named = 0, kept = 0, missing = 0;
        for (String line : Files.readAllLines(in.toPath(), StandardCharsets.UTF_8)) {
            String[] p = line.split(",", 3);
            if (p.length < 2) continue;
            Function f = getFunctionAt(toAddr(p[0]));
            if (f == null) { missing++; continue; }
            if (f.getSymbol().getSource() == SourceType.USER_DEFINED && !f.getName().startsWith("FUN_")) {
                kept++;
            } else {
                f.setName(p[1], SourceType.IMPORTED);
                named++;
            }
            if (p.length == 3 && !p[2].isBlank()) {
                setPlateComment(f.getEntryPoint(), p[2].replaceAll("^\"|\"$", ""));
            }
        }
        println("named " + named + ", kept " + kept + ", missing " + missing);
    }
}
