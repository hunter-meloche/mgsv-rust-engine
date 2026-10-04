// Find the functions that register native Lua modules and dump their
// instructions, so tools/lua_bindings.py can rebuild the binding tables with
// a small register-tracking pass.
//
// Input : a text file of module names, one per line (default
//         re/out/lua_modules.txt, produced by tools/lua_bindings.py).
// Output: re/out/lua_bindings.jsonl, one JSON object per registering function:
//   {"func":"140123450","modules":["TppScriptVars"],
//    "ins":[["MOV",["R9D","0x1"],null,null,0], ["LEA",["RDX","[0x14228fbe8]"],"TYPE_BOOL",null,0],
//           ["CALL",["0x141a156e0"],null,"141a156e0",1], ...]}
// Each instruction is [mnemonic, operands, string_ref, function_ref, is_call, table]
// where table lists the strings of a pointer table the instruction references
// (enum names registered in a loop), or null.
//@category MGSV

import java.io.File;
import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;

import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Data;
import ghidra.program.model.listing.DataIterator;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.Instruction;
import ghidra.program.model.listing.InstructionIterator;
import ghidra.program.model.symbol.Reference;

public class ExportLuaBindings extends GhidraScript {

    private static String js(String s) {
        if (s == null) return "null";
        StringBuilder b = new StringBuilder("\"");
        for (char c : s.toCharArray()) {
            if (c == '"' || c == '\\') b.append('\\').append(c);
            else if (c < 0x20) b.append(String.format("\\u%04x", (int) c));
            else b.append(c);
        }
        return b.append('"').toString();
    }

    private String stringAt(Address a) {
        Data d = getDataAt(a);
        if (d != null && d.hasStringValue()) {
            Object v = d.getValue();
            return v == null ? null : v.toString();
        }
        return null;
    }

    /** Strings pointed to by consecutive 8-byte slots at a, or null if fewer than two. */
    private List<String> stringTable(Address a) {
        List<String> out = new ArrayList<>();
        try {
            for (int k = 0; k < 1024; k++) {
                long p = getLong(a.add(8L * k));
                Address t = toAddr(p);
                String s = t == null ? null : stringAt(t);
                if (s == null) break;
                out.add(s);
            }
        } catch (Exception e) {
            // ran off the end of memory
        }
        return out.size() >= 2 ? out : null;
    }

    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        File in = new File(args.length > 0 ? args[0] : "re/out/lua_modules.txt");
        File out = new File(args.length > 1 ? args[1] : "re/out/lua_bindings.jsonl");
        Set<String> modules = new HashSet<>();
        for (String l : Files.readAllLines(in.toPath(), StandardCharsets.UTF_8)) {
            if (!l.isBlank()) modules.add(l.trim());
        }

        // Module-name string -> functions that reference it.
        Map<Function, Set<String>> registrars = new LinkedHashMap<>();
        DataIterator data = currentProgram.getListing().getDefinedData(true);
        while (data.hasNext() && !monitor.isCancelled()) {
            Data d = data.next();
            if (!d.hasStringValue()) continue;
            Object v = d.getValue();
            if (v == null || !modules.contains(v.toString())) continue;
            for (Reference r : getReferencesTo(d.getAddress())) {
                Function f = getFunctionContaining(r.getFromAddress());
                if (f != null) registrars.computeIfAbsent(f, k -> new HashSet<>()).add(v.toString());
            }
        }
        println("registering functions: " + registrars.size());

        out.getParentFile().mkdirs();
        try (PrintWriter w = new PrintWriter(out, "UTF-8")) {
            for (Map.Entry<Function, Set<String>> e : registrars.entrySet()) {
                if (monitor.isCancelled()) break;
                Function f = e.getKey();
                List<String> ins = new ArrayList<>();
                InstructionIterator it = currentProgram.getListing().getInstructions(f.getBody(), true);
                while (it.hasNext()) {
                    Instruction i = it.next();
                    String str = null, fn = null;
                    List<String> table = null;
                    for (Reference r : i.getReferencesFrom()) {
                        Address to = r.getToAddress();
                        String s = stringAt(to);
                        if (s != null) str = s;
                        else if (getFunctionAt(to) != null) fn = to.toString();
                        else if (i.getMnemonicString().equals("LEA") && to.isMemoryAddress()) table = stringTable(to);
                    }
                    String tbl = "null";
                    if (table != null) {
                        List<String> q = new ArrayList<>();
                        for (String t : table) q.add(js(t));
                        tbl = "[" + String.join(",", q) + "]";
                    }
                    List<String> ops = new ArrayList<>();
                    for (int op = 0; op < i.getNumOperands(); op++) {
                        ops.add(js(i.getDefaultOperandRepresentation(op)));
                    }
                    boolean call = i.getFlowType().isCall();
                    ins.add("[" + js(i.getMnemonicString()) + ",[" + String.join(",", ops) + "],"
                        + js(str) + "," + js(fn) + "," + (call ? 1 : 0) + "," + tbl + "]");
                }
                List<String> mods = new ArrayList<>();
                for (String m : e.getValue()) mods.add(js(m));
                w.printf("{\"func\":\"%s\",\"modules\":[%s],\"ins\":[%s]}%n",
                    f.getEntryPoint(), String.join(",", mods), String.join(",", ins));
            }
        }
        println("wrote " + out.getAbsolutePath());
    }
}
