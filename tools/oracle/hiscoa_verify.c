/*
 * hiscoa_verify — cross-check a golden HSC1 file with captdriver's own decoder
 * (tests/hiscoa-decompress.c, GPL-3.0; this tool is GPL-3.0-or-later).
 *
 * Every band must decode completely (no bytes left) to exactly the rows of the
 * source PBM. This proves the oracle harness called the compressor correctly.
 *
 * usage: hiscoa_verify page.pbm page.hiscoa
 */
#include "hiscoa-decompress.h"

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int read_num(FILE *f, unsigned *out)
{
	int c;
	do {
		c = fgetc(f);
		if (c == '#')
			while (c != '\n' && c != EOF)
				c = fgetc(f);
	} while (c == ' ' || c == '\t' || c == '\n' || c == '\r');
	if (c < '0' || c > '9')
		return -1;
	*out = 0;
	while (c >= '0' && c <= '9') {
		*out = *out * 10 + (unsigned) (c - '0');
		c = fgetc(f);
	}
	return 0;
}

static uint32_t get_u32(const uint8_t *p)
{
	return p[0] | (p[1] << 8) | (p[2] << 16) | ((uint32_t) p[3] << 24);
}

int main(int argc, char **argv)
{
	if (argc != 3) {
		fprintf(stderr, "usage: %s page.pbm page.hiscoa\n", argv[0]);
		return 2;
	}
	FILE *in = fopen(argv[1], "rb");
	unsigned width, height;
	if (!in || fgetc(in) != 'P' || fgetc(in) != '4' || read_num(in, &width) || read_num(in, &height)) {
		fprintf(stderr, "%s: bad PBM\n", argv[1]);
		return 1;
	}
	unsigned line_size = (width + 7) / 8;
	uint8_t *page = malloc((size_t) line_size * height);
	if (!page || fread(page, line_size, height, in) != height)
		return 1;
	fclose(in);

	FILE *g = fopen(argv[2], "rb");
	if (!g)
		return 1;
	fseek(g, 0, SEEK_END);
	long glen = ftell(g);
	fseek(g, 0, SEEK_SET);
	uint8_t *gdata = malloc(glen);
	if (!gdata || fread(gdata, 1, glen, g) != (size_t) glen || memcmp(gdata, "HSC1", 4))
		return 1;
	fclose(g);

	unsigned band_lines = get_u32(gdata + 12), nbands = get_u32(gdata + 16);
	size_t cap = (size_t) line_size * band_lines;
	uint8_t *out = malloc(cap);
	size_t pos = 20;
	int errors = 0, padded = 0;
	/* messages go to stdout: the captdriver decoder traces every command on stderr */
	for (unsigned i = 0; i < nbands; ++i) {
		size_t len = get_u32(gdata + pos);
		pos += 4;
		const void *src = gdata + pos;
		size_t left = len, outsize = cap;
		unsigned start = i * band_lines;
		unsigned nlines = height - start < band_lines ? height - start : band_lines;
		memset(out, 0xAA, cap);
		hiscoa_decompress_band(&src, &left, out, &outsize, line_size, &hiscoa_default_params);
		if (left == 4 && !memcmp(src, "\xBC\xBC\xBC\xBC", 4)) {
			/* captdriver quirk (hiscoa-compress.c:242, `if` commented out): a stream
			 * already 32-bit aligned after END gets one more word of 1-bits,
			 * which is BC BC BC BC after the XOR 0x43. Not an error. */
			padded++;
		} else if (left) {
			printf("band %u: %zu bytes not consumed\n", i, left);
			errors++;
		}
		if (outsize != (size_t) nlines * line_size) {
			printf("band %u: decoded %zu bytes, expected %zu\n", i, outsize,
					(size_t) nlines * line_size);
			errors++;
		} else if (memcmp(out, page + (size_t) start * line_size, outsize)) {
			printf("band %u: decoded rows differ from the PBM\n", i);
			errors++;
		}
		pos += len;
	}
	printf("%s: %u bands, %d with extra padding word, %d errors\n", argv[2], nbands, padded, errors);
	return errors ? 1 : 0;
}
