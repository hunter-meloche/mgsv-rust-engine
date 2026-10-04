// Pre-analysis setup for mgsvtpp.exe.
//
// The retail executable carries a protector layer in `.xdata` (117 MB, RWX,
// holds the PE entry point). The engine itself lives in `.xtext` as plain
// x64 code. We drop the protector block so auto-analysis only spends time on
// engine code; we never analyse, modify or bypass the protector.
//@category MGSV

import ghidra.app.script.GhidraScript;
import ghidra.program.model.mem.Memory;
import ghidra.program.model.mem.MemoryBlock;

public class PrepareMgsv extends GhidraScript {
    @Override
    public void run() throws Exception {
        Memory mem = currentProgram.getMemory();
        MemoryBlock b = mem.getBlock(".xdata");
        if (b != null) {
            mem.removeBlock(b, monitor);
            println("removed protector block .xdata");
        }
        // .xtext is the real text section; make sure it is executable.
        MemoryBlock text = mem.getBlock(".xtext");
        if (text != null) {
            text.setExecute(true);
        }
    }
}
