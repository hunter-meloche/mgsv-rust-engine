// Find the functions that register native Lua modules and dump what they
// reference, in instruction order, so the binding tables can be rebuilt.
//
// Input : a text file of module names, one per line (default
//         re/out/lua_modules.txt, produced by tools/lua_bindings.py).
// Output: re/out/lua_bindings.jsonl, one JSON object per registering function:
//   {"func":"140123450","name":"FUN_140123450","modules":["TppScriptVars"],
//    "events":[["s","DeclareGVars"],["f","140456780"],["i",3],...]}
// Event kinds: s = string literal, f = referenced function entry,
//              i = scalar immediate operand.
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
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.Instruction;
import ghidra.program.model.listing.InstructionIterator;
import ghidra.program.model.scalar.Scalar;
import ghidra.program.model.symbol.Reference;
import ghidra.program.util.DefinedDataIterator;

public class ExportLuaBindings extends GhidraScript {

    private static String esc(String s) {
        StringBuilder b = new StringBuilder();
        for (char c : s.toCharArray()) {
            if (c == '"' || c == '\\') b.append('\\').append(c);
            else if (c < 0x20) b.append(String.format("\\u%04x", (int) c));
            else b.append(c);
        }
        return b.toString();
    }

    private String stringAt(Address a) {
        Data d = getDataAt(a);
        if (d != null && d.hasStringValue()) {
            Object v = d.getValue();
            return v == null ? null : v.toString();
        }
        return null;
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
        for (Data d : DefinedDataIterator.definedStrings(currentProgram)) {
            if (monitor.isCancelled()) break;
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
                List<String> events = new ArrayList<>();
                InstructionIterator it = currentProgram.getListing().getInstructions(f.getBody(), true);
                while (it.hasNext()) {
                    Instruction ins = it.next();
                    for (Reference r : ins.getReferencesFrom()) {
                        Address to = r.getToAddress();
                        String s = stringAt(to);
                        if (s != null) {
                            events.add("[\"s\",\"" + esc(s) + "\"]");
                            continue;
                        }
                        Function callee = getFunctionAt(to);
                        if (callee != null && !r.getReferenceType().isCall()) {
                            events.add("[\"f\",\"" + to + "\"]");
                        } else if (callee != null) {
                            events.add("[\"c\",\"" + to + "\"]");
                        }
                    }
                    for (int op = 0; op < ins.getNumOperands(); op++) {
                        for (Object o : ins.getOpObjects(op)) {
                            if (o instanceof Scalar sc && ins.getReferencesFrom().length == 0) {
                                events.add("[\"i\"," + sc.getSignedValue() + "]");
                            }
                        }
                    }
                }
                StringBuilder mods = new StringBuilder();
                for (String m : e.getValue()) {
                    if (mods.length() > 0) mods.append(',');
                    mods.append('"').append(esc(m)).append('"');
                }
                w.printf("{\"func\":\"%s\",\"name\":\"%s\",\"modules\":[%s],\"events\":[%s]}%n",
                    f.getEntryPoint(), esc(f.getName()), mods, String.join(",", events));
            }
        }
        println("wrote " + out.getAbsolutePath());
    }
}
