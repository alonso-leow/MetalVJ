package com.metalvj.dash;

import android.Manifest;
import android.app.NativeActivity;
import android.os.Bundle;

/**
 * Thin NativeActivity subclass that requests the runtime permissions the
 * native shell needs for capture (audio input, camera). The Rust side
 * (android_main in metal_dash) is unchanged; the lib_name meta-data still
 * selects libmetal_dash.so.
 */
public class MainActivity extends NativeActivity {
    private static final int PERMISSION_REQUEST = 1;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        requestPermissions(
                new String[] { Manifest.permission.RECORD_AUDIO, Manifest.permission.CAMERA },
                PERMISSION_REQUEST);
    }
}
