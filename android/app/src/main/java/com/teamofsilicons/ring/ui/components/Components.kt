package com.teamofsilicons.ring.ui.components

import androidx.annotation.DrawableRes
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.draw.scale
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import coil3.compose.AsyncImage
import com.teamofsilicons.ring.data.Account
import com.teamofsilicons.ring.ui.theme.HedvigSerif
import com.teamofsilicons.ring.ui.theme.Ring
import com.teamofsilicons.ring.ui.theme.RingType

/** One physical pixel, the native hairline. */
val hairline: Dp
    @Composable get() = with(LocalDensity.current) { 1f.toDp() }

@Composable
fun RingIcon(@DrawableRes icon: Int, contentDescription: String?, modifier: Modifier = Modifier, tint: Color = Ring.colors.foreground, size: Dp = 22.dp) {
    Icon(painterResource(icon), contentDescription, modifier.size(size), tint = tint)
}

/** Small uppercase label in Geist Pixel Square. */
@Composable
fun PixelLabel(
    text: String,
    modifier: Modifier = Modifier,
    color: Color = Ring.colors.textMuted,
    style: TextStyle = RingType.pixel,
    uppercase: Boolean = true,
) {
    Text(if (uppercase) text.uppercase() else text, modifier = modifier, style = style, color = color, maxLines = 1, overflow = TextOverflow.Ellipsis)
}

/** An account id (`c:alice`, `si:tos`) in the pixel face; ids keep their case. */
@Composable
fun PixelId(id: String, modifier: Modifier = Modifier, color: Color = Ring.colors.textMuted, style: TextStyle = RingType.pixel) {
    PixelLabel(id, modifier, color, style, uppercase = false)
}

/** The SILICON / CARBON / ON RING badges (`.wt-badge`). */
@Composable
fun Badge(text: String, modifier: Modifier = Modifier, accent: Boolean = false) {
    val c = Ring.colors
    Box(
        modifier
            .height(22.dp)
            .clip(RoundedCornerShape(5.dp))
            .background(if (accent) c.accentSubtle else c.beige)
            .border(hairline, if (accent) c.accentBorder else c.borderStrong, RoundedCornerShape(5.dp))
            .padding(horizontal = 7.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(text.uppercase(), style = RingType.pixelSmall, color = if (accent) c.accent else c.foreground, maxLines = 1)
    }
}

@Composable
fun SiliconBadge(account: Account, modifier: Modifier = Modifier) {
    if (account.isSilicon) Badge("Silicon", modifier, accent = true)
}

/** A circular avatar from `pfp_url`, falling back to the serif initial on beige. */
@Composable
fun Avatar(account: Account?, size: Dp, modifier: Modifier = Modifier) {
    val c = Ring.colors
    val label = account?.label ?: "?"
    Box(
        modifier
            .size(size)
            .clip(CircleShape)
            .background(c.beige)
            .border(hairline, c.borderStrong, CircleShape),
        contentAlignment = Alignment.Center,
    ) {
        val letter = label.trim().removePrefix("si:").removePrefix("c:").firstOrNull()?.uppercase() ?: "?"
        Text(
            letter,
            style = TextStyle(fontFamily = HedvigSerif, fontSize = (size.value * 0.44f).sp, lineHeight = (size.value * 0.5f).sp),
            color = c.accent,
        )
        if (!account?.pfpUrl.isNullOrBlank()) {
            AsyncImage(
                model = account.pfpUrl,
                contentDescription = null,
                contentScale = ContentScale.Crop,
                modifier = Modifier.size(size).clip(CircleShape),
            )
        }
    }
}

/** The Ring mark: concentric cream rings around a cream dot, on ink. */
@Composable
fun RingMark(size: Dp, modifier: Modifier = Modifier, ink: Color = Color(0xFF181818), cream: Color = Color(0xFFEDE8E0), rounded: Boolean = true) {
    Canvas(
        modifier
            .size(size)
            .clip(if (rounded) RoundedCornerShape(size * 0.22f) else RoundedCornerShape(0.dp)),
    ) {
        val unit = this.size.minDimension / 200f
        val center = Offset(this.size.width / 2f, this.size.height / 2f)
        drawRect(ink)
        drawCircle(cream, radius = 11.5f * unit, center = center)
        drawCircle(cream, radius = 32f * unit, center = center, style = Stroke(3f * unit))
        drawOval(
            cream,
            topLeft = Offset(center.x - 44.5f * unit, center.y - 43.5f * unit),
            size = androidx.compose.ui.geometry.Size(89f * unit, 87f * unit),
            style = Stroke(2f * unit),
        )
        drawCircle(cream, radius = 55f * unit, center = center, style = Stroke(1.2f * unit))
    }
}

/** The mark drawn as bare rings in one colour (empty states). */
@Composable
fun RingRings(size: Dp, color: Color, modifier: Modifier = Modifier) {
    Canvas(modifier.size(size)) {
        val unit = this.size.minDimension / 120f
        val center = Offset(this.size.width / 2f, this.size.height / 2f)
        drawCircle(color, radius = 11.5f * unit, center = center)
        drawCircle(color, radius = 32f * unit, center = center, style = Stroke(3f * unit))
        drawCircle(color, radius = 55f * unit, center = center, style = Stroke(1.5f * unit))
    }
}

/** A list group card: surface fill, radius 12, hairline border, soft shadow. */
@Composable
fun CardGroup(modifier: Modifier = Modifier, content: @Composable ColumnScope.() -> Unit) {
    val c = Ring.colors
    Column(
        modifier
            .fillMaxWidth()
            .shadow(if (c.dark) 0.dp else 2.dp, RoundedCornerShape(12.dp), ambientColor = Color.Black.copy(alpha = 0.06f), spotColor = Color.Black.copy(alpha = 0.06f))
            .clip(RoundedCornerShape(12.dp))
            .background(c.surface)
            .border(hairline, c.border, RoundedCornerShape(12.dp)),
        content = content,
    )
}

/**
 * One row of a card group rendered as separate lazy items: surface fill and hairline sides, with
 * the rounded top on the first row and the rounded bottom on the last.
 */
@Composable
fun Modifier.cardSegment(first: Boolean, last: Boolean): Modifier {
    val c = Ring.colors
    val line = hairline
    val shape = RoundedCornerShape(
        topStart = if (first) 12.dp else 0.dp,
        topEnd = if (first) 12.dp else 0.dp,
        bottomStart = if (last) 12.dp else 0.dp,
        bottomEnd = if (last) 12.dp else 0.dp,
    )
    return this
        .clip(shape)
        .background(c.surface)
        .drawWithContent {
            drawContent()
            val stroke = line.toPx()
            val r = 12.dp.toPx()
            val w = size.width
            val h = size.height
            val top = if (first) r else 0f
            val bottom = if (last) h - r else h
            val half = stroke / 2f
            drawLine(c.border, Offset(half, top), Offset(half, bottom), stroke)
            drawLine(c.border, Offset(w - half, top), Offset(w - half, bottom), stroke)
            val arc = Size(2 * r - stroke, 2 * r - stroke)
            if (first) {
                drawArc(c.border, 180f, 90f, false, Offset(half, half), arc, style = Stroke(stroke))
                drawArc(c.border, 270f, 90f, false, Offset(w - 2 * r + half, half), arc, style = Stroke(stroke))
                drawLine(c.border, Offset(r, half), Offset(w - r, half), stroke)
            }
            if (last) {
                drawArc(c.border, 90f, 90f, false, Offset(half, h - 2 * r + half), arc, style = Stroke(stroke))
                drawArc(c.border, 0f, 90f, false, Offset(w - 2 * r + half, h - 2 * r + half), arc, style = Stroke(stroke))
                drawLine(c.border, Offset(r, h - half), Offset(w - r, h - half), stroke)
            }
        }
}

@Composable
fun Divider(modifier: Modifier = Modifier, inset: Dp = 0.dp) {
    Box(
        modifier
            .padding(start = inset)
            .fillMaxWidth()
            .height(hairline)
            .background(Ring.colors.border),
    )
}

/** Section header above a card group. */
@Composable
fun SectionHeader(text: String, modifier: Modifier = Modifier) {
    PixelLabel(text, modifier.padding(start = 4.dp, top = 20.dp, bottom = 8.dp))
}

/** A tappable row inside a card group. */
@Composable
fun GroupRow(
    modifier: Modifier = Modifier,
    onClick: (() -> Unit)? = null,
    padding: PaddingValues = PaddingValues(horizontal = 16.dp, vertical = 12.dp),
    content: @Composable RowScope.() -> Unit,
) {
    Row(
        modifier
            .fillMaxWidth()
            .heightIn(min = 52.dp)
            .then(if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier)
            .padding(padding),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        content = content,
    )
}

private val buttonShape = RoundedCornerShape(8.dp)

@Composable
private fun pressScale(source: MutableInteractionSource): Float {
    val pressed by source.collectIsPressedAsState()
    return animateFloatAsState(if (pressed) 0.98f else 1f, tween(150), label = "press").value
}

/** Filled blue accent button with white text. */
@Composable
fun PrimaryButton(
    text: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    @DrawableRes icon: Int? = null,
    color: Color = Ring.colors.accent,
) {
    val c = Ring.colors
    val source = remember { MutableInteractionSource() }
    val fill by animateColorAsState(if (enabled) color else color.copy(alpha = 0.45f), tween(150), label = "fill")
    Row(
        modifier
            .scale(pressScale(source))
            .heightIn(min = 44.dp)
            .shadow(if (enabled && !c.dark) 3.dp else 0.dp, buttonShape)
            .clip(buttonShape)
            .background(fill)
            .border(hairline, color.copy(alpha = 0.6f), buttonShape)
            .clickable(source, null, enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(horizontal = 18.dp, vertical = 11.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.Center,
    ) {
        if (icon != null) {
            RingIcon(icon, null, tint = c.onPrimary, size = 18.dp)
            Spacer(Modifier.width(8.dp))
        }
        Text(text, style = RingType.button, color = c.onPrimary, maxLines = 1)
    }
}

/** White (70 %) secondary button with a hairline border. */
@Composable
fun SecondaryButton(
    text: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    @DrawableRes icon: Int? = null,
    contentColor: Color = Ring.colors.foreground,
) {
    val c = Ring.colors
    val source = remember { MutableInteractionSource() }
    Row(
        modifier
            .scale(pressScale(source))
            .heightIn(min = 44.dp)
            .clip(buttonShape)
            .background(if (c.dark) c.surfaceRaised else Color.White.copy(alpha = 0.7f))
            .border(hairline, if (c.dark) c.borderStrong else Color(0xFFE5E5E5), buttonShape)
            .clickable(source, null, enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(horizontal = 16.dp, vertical = 11.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.Center,
    ) {
        if (icon != null) {
            RingIcon(icon, null, tint = contentColor, size = 18.dp)
            Spacer(Modifier.width(8.dp))
        }
        Text(text, style = RingType.button, color = if (enabled) contentColor else contentColor.copy(alpha = 0.5f), maxLines = 1)
    }
}

/** Plain text button in the accent colour. */
@Composable
fun TextAction(text: String, onClick: () -> Unit, modifier: Modifier = Modifier, color: Color = Ring.colors.accentText) {
    Text(
        text,
        style = RingType.button,
        color = color,
        modifier = modifier
            .clip(buttonShape)
            .clickable(role = Role.Button, onClick = onClick)
            .padding(horizontal = 10.dp, vertical = 10.dp),
    )
}

/** Round call control (mute, route, add, hang up). */
@Composable
fun RoundButton(
    @DrawableRes icon: Int,
    contentDescription: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    size: Dp = 60.dp,
    background: Color,
    tint: Color,
    border: BorderStroke? = null,
    enabled: Boolean = true,
) {
    val source = remember { MutableInteractionSource() }
    Box(
        modifier
            .scale(pressScale(source))
            .size(size)
            .clip(CircleShape)
            .background(background)
            .then(if (border != null) Modifier.border(border, CircleShape) else Modifier)
            .clickable(source, null, enabled = enabled, role = Role.Button, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        RingIcon(icon, contentDescription, tint = if (enabled) tint else tint.copy(alpha = 0.4f), size = size * 0.42f)
    }
}

/** A small round icon button for rows (redial, play). */
@Composable
fun IconAction(@DrawableRes icon: Int, contentDescription: String, onClick: () -> Unit, modifier: Modifier = Modifier, tint: Color = Ring.colors.accent, enabled: Boolean = true) {
    Box(
        modifier
            .size(40.dp)
            .clip(CircleShape)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        RingIcon(icon, contentDescription, tint = if (enabled) tint else tint.copy(alpha = 0.4f), size = 22.dp)
    }
}

/** A serif display name with the pixel id below it (or the id large when there is no name). */
@Composable
fun Identity(account: Account, nameStyle: TextStyle, nameColor: Color, idColor: Color, modifier: Modifier = Modifier, center: Boolean = true) {
    Column(modifier, horizontalAlignment = if (center) Alignment.CenterHorizontally else Alignment.Start) {
        Text(
            account.label,
            style = nameStyle,
            color = nameColor,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
            textAlign = if (center) androidx.compose.ui.text.style.TextAlign.Center else null,
        )
        if (account.displayName?.isNotBlank() == true) {
            Spacer(Modifier.height(6.dp))
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                PixelId(account.id, color = idColor, style = RingType.pixelLarge)
                SiliconBadge(account)
            }
        } else if (account.isSilicon) {
            Spacer(Modifier.height(6.dp))
            SiliconBadge(account)
        }
    }
}
