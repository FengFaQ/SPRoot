package com.sproot.manager

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.ui.Modifier
import com.sproot.manager.ui.AppRoot

class MainActivity : ComponentActivity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // 边到边：Miuix Scaffold 自行处理 systemBars insets
        enableEdgeToEdge()
        setContent {
            AppRoot(modifier = Modifier.fillMaxSize())
        }
    }
}
