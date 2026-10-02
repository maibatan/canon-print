/*
 * hiscoa_oracle — reference Hi-SCoA encoder for golden tests.
 *
 * Links captdriver's src/hiscoa-compress.c + src/hiscoa-common.c (GPL-3.0),
 * therefore this tool is GPL-3.0-or-later. It is a development tool only and is
 * not part of the canon-print binary.
 *
 * Reads a PBM (P4) page and writes an HSC1 file (see docs/testing.md), calling
 * hiscoa_compress_band() exactly like captdriver's rastertocapt.c:
 *   - bands of 70 lines, the last band shorter (nlines < 70)
 *   - the band buffer is zeroed to line_size * 70 before copying rows
 *   - output buffer size 2 * line_size * 70
 *   - HISCOA_EOB_NORMAL for every band (generic-ops.c), default params
 *
 * usage: hiscoa_oracle in.pbm out.hiscoa
 */
#include "hiscoa-compress.h"

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define BAND_LINES 70

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
	return 0; /* the single whitespace after the number is consumed */
}

static void put_u32(FILE *f, uint32_t v)
{
	uint8_t b[4] = { v & 0xFF, (v >> 8) & 0xFF, (v >> 16) & 0xFF, (v >> 24) & 0xFF };
	fwrite(b, 1, 4, f);
}

int main(int argc, char **argv)
{
	if (argc != 3) {
		fprintf(stderr, "usage: %s in.pbm out.hiscoa\n", argv[0]);
		return 2;
	}
	FILE *in = fopen(argv[1], "rb");
	if (!in) {
		perror(argv[1]);
		return 1;
	}
	if (fgetc(in) != 'P' || fgetc(in) != '4') {
		fprintf(stderr, "%s: not a P4 PBM\n", argv[1]);
		return 1;
	}
	unsigned width, height;
	if (read_num(in, &width) || read_num(in, &height) || !width || !height) {
		fprintf(stderr, "%s: bad PBM header\n", argv[1]);
		return 1;
	}
	unsigned line_size = (width + 7) / 8;
	uint8_t *page = malloc((size_t) line_size * height);
	if (!page || fread(page, line_size, height, in) != height) {
		fprintf(stderr, "%s: truncated PBM data\n", argv[1]);
		return 1;
	}
	fclose(in);

	unsigned nbands = (height + BAND_LINES - 1) / BAND_LINES;
	size_t compsize = 2 * (size_t) line_size * BAND_LINES;
	uint8_t *bandbuf = malloc((size_t) line_size * BAND_LINES);
	uint8_t *comp = malloc(compsize);
	if (!bandbuf || !comp)
		return 1;

	FILE *out = fopen(argv[2], "wb");
	if (!out) {
		perror(argv[2]);
		return 1;
	}
	fwrite("HSC1", 1, 4, out);
	put_u32(out, line_size);
	put_u32(out, height);
	put_u32(out, BAND_LINES);
	put_u32(out, nbands);

	for (unsigned iband = 0; iband < nbands; ++iband) {
		unsigned start = iband * BAND_LINES;
		unsigned nlines = height - start < BAND_LINES ? height - start : BAND_LINES;
		memset(bandbuf, 0, (size_t) line_size * BAND_LINES);
		memcpy(bandbuf, page + (size_t) start * line_size, (size_t) nlines * line_size);
		size_t size = hiscoa_compress_band(comp, compsize, bandbuf, line_size, nlines,
				HISCOA_EOB_NORMAL, &hiscoa_default_params);
		put_u32(out, (uint32_t) size);
		fwrite(comp, 1, size, out);
	}
	if (fclose(out)) {
		perror(argv[2]);
		return 1;
	}
	free(page);
	free(bandbuf);
	free(comp);
	return 0;
}
