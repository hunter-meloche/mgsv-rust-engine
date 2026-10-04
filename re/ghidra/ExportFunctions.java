// Export every function as CSV: entry,size,name,callers,callees.
// Output: re/out/functions.csv (gitignored).
//@category MGSV

import java.io.File;
import java.io.PrintWriter;

import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.FunctionIterator;

public class ExportFunctions extends GhidraScript {
    @Override
    public void run() throws Exception {
        File out = new File(getScriptArgs().length > 0 ? getScriptArgs()[0] : "re/out/functions.csv");
        out.getParentFile().mkdirs();
        int n = 0;
        try (PrintWriter w = new PrintWriter(out, "UTF-8")) {
            w.println("entry,size,name,callers,callees");
            FunctionIterator it = currentProgram.getFunctionManager().getFunctions(true);
            while (it.hasNext() && !monitor.isCancelled()) {
                Function f = it.next();
                w.printf("%s,%d,%s,%d,%d%n",
                    f.getEntryPoint(),
                    f.getBody().getNumAddresses(),
                    f.getName(true).replace(',', ';'),
                    f.getCallingFunctions(monitor).size(),
                    f.getCalledFunctions(monitor).size());
                n++;
            }
        }
        println("wrote " + n + " functions to " + out.getAbsolutePath());
    }
}
