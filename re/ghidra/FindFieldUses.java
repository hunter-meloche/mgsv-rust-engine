// List instructions that use a structure field offset (e.g. "+ 0x868]"),
// optionally limited to an address range, grouped by function.
//
// Args: <offset hex> [<start hex> <end hex>] [<output file>]
// Output: re/out/field_uses.txt (gitignored)
//@category MGSV

import java.io.File;
import java.io.PrintWriter;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.ArrayList;
import java.util.Map;

import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.address.AddressSet;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.Instruction;
import ghidra.program.model.listing.InstructionIterator;

public class FindFieldUses extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        String needle = "+ 0x" + a[0].toLowerCase().replaceFirst("^0x", "") + "]";
        AddressSet range = new AddressSet(currentProgram.getMemory());
        if (a.length >= 3) {
            range = new AddressSet(toAddr(a[1]), toAddr(a[2]));
        }
        File out = new File(a.length >= 4 ? a[3] : "re/out/field_uses.txt");
        Map<Function, List<String>> hits = new LinkedHashMap<>();
        InstructionIterator it = currentProgram.getListing().getInstructions(range, true);
        while (it.hasNext() && !monitor.isCancelled()) {
            Instruction i = it.next();
            if (!i.toString().toLowerCase().contains(needle)) continue;
            Function f = getFunctionContaining(i.getAddress());
            if (f == null) continue;
            hits.computeIfAbsent(f, k -> new ArrayList<>()).add(i.getAddress() + "  " + i);
        }
        out.getParentFile().mkdirs();
        try (PrintWriter w = new PrintWriter(out, "UTF-8")) {
            for (Map.Entry<Function, List<String>> e : hits.entrySet()) {
                w.println(e.getKey().getEntryPoint() + " " + e.getKey().getName() + " (" + e.getValue().size() + ")");
                for (String s : e.getValue()) w.println("    " + s);
            }
        }
        println("functions using " + needle + ": " + hits.size() + " -> " + out.getAbsolutePath());
    }
}
