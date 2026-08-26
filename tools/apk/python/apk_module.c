/*
 * Copyright (C) 2025 apk-tools authors
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#define PY_SSIZE_T_CLEAN
#include <Python.h>
#include "apk_blob.h"
#include "apk_version.h"

static apk_blob_t python_str_to_blob(PyObject *py_str) {
	const char *str;
	Py_ssize_t len;
	str = PyUnicode_AsUTF8AndSize(py_str, &len);
	apk_blob_t blob = APK_BLOB_PTR_LEN((char *) str, len);
	return blob;
}

/* version_validate(verstr) -> bool */
static PyObject *version_validate(PyObject *self, PyObject *args) {
	PyObject *py_verstr;
	if (!PyArg_ParseTuple(args, "U", &py_verstr)) {
		return NULL;
	}

	apk_blob_t ver = python_str_to_blob(py_verstr);
	int result = apk_version_validate(ver);
	return PyBool_FromLong(result);
}

/* version_compare(verstr1, verstr2) -> int */
static PyObject *version_compare(PyObject *self, PyObject *args) {
	PyObject *py_verstr1, *py_verstr2;
	if (!PyArg_ParseTuple(args, "UU", &py_verstr1, &py_verstr2)) {
		return NULL;
	}

	apk_blob_t ver1 = python_str_to_blob(py_verstr1);
	apk_blob_t ver2 = python_str_to_blob(py_verstr2);

	return PyLong_FromLong(apk_version_compare(ver1, ver2));
}

/* version_match(verstr1, op, verstr2) -> bool */
static PyObject *version_match(PyObject *self, PyObject *args) {
	PyObject *py_verstr1, *py_verstr2;
	int op;

	if (!PyArg_ParseTuple(args, "UiU", &py_verstr1, &op, &py_verstr2)) {
		return NULL;
	}

	apk_blob_t ver1 = python_str_to_blob(py_verstr1);
	apk_blob_t ver2 = python_str_to_blob(py_verstr2);

	int result = apk_version_match(ver1, op, ver2);
	return PyBool_FromLong(result);
}

static PyMethodDef ApkVersionMethods[] = {
	{"validate", version_validate, METH_VARARGS, "Validate a version string."},
	{"compare", version_compare, METH_VARARGS, "Compare two version strings. Returns an integer."},
	{"match", version_match, METH_VARARGS, "Match two version strings with a specified operation."},
	{NULL, NULL, 0, NULL}
};

static struct PyModuleDef apkversionmodule = {
	PyModuleDef_HEAD_INIT,
	"apk.version",
	"Namespace for version-related functions in libapk.",
	-1,
	ApkVersionMethods
};

static PyObject *PyInit_apk_version(void) {
	PyObject *version_module = PyModule_Create(&apkversionmodule);
	if (!version_module) {
		return NULL;
	}

	PyModule_AddIntConstant(version_module, "UNKNOWN", APK_VERSION_UNKNOWN);
	PyModule_AddIntConstant(version_module, "EQUAL", APK_VERSION_EQUAL);
	PyModule_AddIntConstant(version_module, "LESS", APK_VERSION_LESS);
	PyModule_AddIntConstant(version_module, "GREATER", APK_VERSION_GREATER);
	PyModule_AddIntConstant(version_module, "FUZZY", APK_VERSION_FUZZY);
	PyModule_AddIntConstant(version_module, "CONFLICT", APK_VERSION_CONFLICT);

	return version_module;
}

static struct PyModuleDef apkmodule = {
	PyModuleDef_HEAD_INIT,
	"apk",
	"Python bindings for libapk functions.",
	-1,
	NULL
};

PyMODINIT_FUNC PyInit_apk(void) {
	PyObject *module = PyModule_Create(&apkmodule);
	if (!module) {
		return NULL;
	}

	PyObject *version_module = PyInit_apk_version();
	if (!version_module) {
		Py_DECREF(module);
		return NULL;
	}
	PyModule_AddObject(module, "version", version_module);

	return module;
}
