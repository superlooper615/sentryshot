// SPDX-License-Identifier: GPL-2.0-or-later

// @ts-check

import { denormalize, htmlToElem, relativePathname, sleep } from "../libs/common.js";

const pollIntervalMs = 500;
const staleAfterMs = 3000;

/**
 * @typedef {Object} Detection
 * @property {string} label
 * @property {Region} region
 * @property {number} score
 */

/**
 * @typedef {Object} Region
 * @property {null} polygon
 * @property {Rectangle} rectangle
 */

/**
 * @typedef {Object} Rectangle
 * @property {number} height
 * @property {number} width
 * @property {number} x
 * @property {number} y
 */

/**
 * @param {Rectangle} rect
 * @param {string} label
 * @param {number} score
 */
function renderRectangle(rect, label, score) {
	const x = denormalize(rect.x, 100);
	const y = denormalize(rect.y, 100);
	const width = denormalize(rect.width, 100);
	const height = denormalize(rect.height, 100);

	const textY = y > 10 ? y - 2 : y + height + 5;
	return `
		<text
			x="${x}"
			y="${textY}"
			font-size="5"
			style="fill-opacity: 1; fill: var(--color-red); stroke-opacity: 0;"
		>
			${label} ${Math.round(score)}%
		</text>
		<rect x="${x}" width="${width}" y="${y}" height="${height}" />
	`;
}

/** @param {Detection[]} detections */
function renderDetections(detections) {
	let html = "";
	if (!detections) {
		return html;
	}
	for (const d of detections) {
		if (d.region && d.region.rectangle) {
			html += renderRectangle(d.region.rectangle, d.label, d.score);
		}
	}
	return html;
}

/**
 * @param {AbortSignal} abortSignal
 * @param {Element} elem
 * @param {string} monitorId
 */
async function pollDetections(abortSignal, elem, monitorId) {
	const url = new URL(relativePathname(`api/monitor/${monitorId}/object-detection/recent`));
	while (!abortSignal.aborted) {
		try {
			const response = await fetch(url, { method: "get", signal: abortSignal });
			if (response.status === 200) {
				const event = await response.json();
				const eventTimeMs = event.time / 1000000;
				const ageMs = Date.now() - eventTimeMs;
				elem.innerHTML = ageMs <= staleAfterMs ? renderDetections(event.detections) : "";
			} else if (response.status === 204 || response.status === 404) {
				elem.innerHTML = "";
			}
		} catch (error) {
			if (!abortSignal.aborted) {
				elem.innerHTML = "";
			}
		}
		await sleep(abortSignal, pollIntervalMs);
	}
}

/**
 * @param {AbortSignal} abortSignal
 * @param {string} monitorId
 */
function newDetectionOverlay(abortSignal, monitorId) {
	const elem = htmlToElem(/* HTML */ `
		<svg
			class="js-live-detections absolute w-full h-full"
			style="
				z-index: 1;
				pointer-events: none;
				stroke: var(--color-red);
				fill-opacity: 0;
				stroke-width: calc(var(--scale) * 0.05rem);
			"
			viewBox="0 0 100 100"
			preserveAspectRatio="none"
		></svg>
	`);
	// @ts-ignore uiData is injected by the Sentryshot template at runtime.
	if (window.uiData !== undefined) {
		pollDetections(abortSignal, elem, monitorId);
	}
	return elem;
}

export { newDetectionOverlay, renderDetections };
