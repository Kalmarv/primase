// Native bcdec oracle: compile in scratch with its bcdec.h on include path.
// Synthetic input only. Usage: native.exe input.blocks output.rgb32f signed
#define BCDEC_IMPLEMENTATION
#include "bcdec.h"
#include <stdio.h>
#include <stdlib.h>
int main(int argc,char**argv) {
    if(argc!=4)return 2;
    FILE* in=fopen(argv[1],"rb");FILE*out=fopen(argv[2],"wb");
    if(!in||!out)return 3;
    unsigned char block[16];float rgb[48];
    while(fread(block,1,16,in)==16){bcdec_bc6h_float(block,rgb,12,atoi(argv[3]));if(fwrite(rgb,sizeof(float),48,out)!=48)return 4;}
    fclose(in);fclose(out);return 0;
}
